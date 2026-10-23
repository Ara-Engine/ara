use crate::{
    BridgeError, Command, Error, LogRecord, PROTOCOL_VERSION, PerfSummary, RuntimeHooks, Telemetry,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
mod connection;

/// Nonblocking bounded handoff from runtime threads to the bridge worker.
#[derive(Clone)]
pub struct Publisher {
    tx: SyncSender<Telemetry>,
    dropped: Arc<AtomicU64>,
}
impl Publisher {
    /// Returns false on backpressure/disconnect; never waits for the network.
    pub fn try_publish(&self, mut batch: Telemetry) -> bool {
        batch.logs.truncate(256);
        batch.frame_us.truncate(256);
        batch.gpu_us.truncate(256);
        for log in &mut batch.logs {
            log.message.truncate(log.message.floor_char_boundary(8192));
        }
        if self.tx.try_send(batch).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            true
        }
    }
}

/// RAII server. Dropping it stops and joins its worker, including stalled clients.
pub struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    publisher: Publisher,
}
impl Server {
    /// Binds loopback only. Pass port zero for an OS-assigned port. Tokens must
    /// contain at least 32 bytes; release players must never call this method.
    pub fn start(
        address: SocketAddr,
        token: String,
        hooks: impl RuntimeHooks,
    ) -> Result<Self, Error> {
        if !address.ip().is_loopback() || token.len() < 32 {
            return Err(Error::Diagnostic(BridgeError::new(
                "ARA-EXP-BRG-004",
                "loopback and a 32-byte token are required",
                "configure a private dev bridge token and localhost bind",
            )));
        }
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let dropped = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::sync_channel(64);
        let publisher = Publisher {
            tx,
            dropped: dropped.clone(),
        };
        let worker = thread::Builder::new()
            .name("ara-bridge".into())
            .spawn(move || {
                let mut state = State {
                    logs: VecDeque::new(),
                    perf: PerfSummary::default(),
                    sequence: 0,
                    hooks,
                    rx,
                    dropped,
                };
                let mut clients = Vec::new();
                while !worker_stop.load(Ordering::Acquire) {
                    state.drain();
                    for _ in 0..8 {
                        match listener.accept() {
                            Ok((stream, _)) if clients.len() < 8 => {
                                if let Ok(client) = connection::Connection::accept(stream) {
                                    clients.push(client);
                                }
                            }
                            Ok(_) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => {
                                worker_stop.store(true, Ordering::Release);
                                break;
                            }
                        }
                    }
                    clients.retain_mut(|client| client.poll(&token, &mut state).unwrap_or(false));
                    thread::sleep(Duration::from_millis(5));
                }
            })?;
        Ok(Self {
            address,
            stop,
            worker: Some(worker),
            publisher,
        })
    }
    /// Actual endpoint, including the assigned port.
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    /// Clones the bounded telemetry channel, never the game world.
    pub fn publisher(&self) -> Publisher {
        self.publisher.clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct State<H> {
    logs: VecDeque<LogRecord>,
    perf: PerfSummary,
    sequence: u64,
    hooks: H,
    rx: Receiver<Telemetry>,
    dropped: Arc<AtomicU64>,
}
impl<H: RuntimeHooks> State<H> {
    fn drain(&mut self) {
        for _ in 0..64 {
            let Ok(batch) = self.rx.try_recv() else { break };
            for mut log in batch.logs {
                self.sequence += 1;
                log.sequence = self.sequence;
                self.logs.push_back(log);
                if self.logs.len() > 2048 {
                    self.logs.pop_front();
                }
            }
            for us in batch.frame_us {
                self.perf.frame.record(us);
            }
            for us in batch.gpu_us {
                self.perf.gpu.record(us);
            }
        }
        self.perf.dropped_batches = self.dropped.load(Ordering::Relaxed);
    }
    fn execute(&mut self, command: Command) -> Result<Value, BridgeError> {
        self.drain();
        match command {
            Command::Hello => Ok(
                json!({"protocol": PROTOCOL_VERSION, "transport": "websocket", "log_capacity": 2048}),
            ),
            Command::Logs { after } => Ok(
                json!({"logs": self.logs.iter().filter(|l| l.sequence > after).collect::<Vec<_>>(), "latest": self.sequence, "oldest": self.logs.front().map(|l| l.sequence)}),
            ),
            Command::Perf => Ok(json!(self.perf)),
            Command::Screenshot => self
                .hooks
                .screenshot()
                .map(|png| json!({"mime": "image/png", "base64": STANDARD.encode(png)})),
            Command::Input(action) => {
                if action.action.is_empty()
                    || action.action.len() > 128
                    || action.values.len() > 4
                    || action
                        .values
                        .iter()
                        .any(|v| !v.is_finite() || v.abs() > 1.0)
                {
                    return Err(BridgeError::new(
                        "ARA-EXP-BRG-014",
                        "invalid input action",
                        "use a declared action and up to four finite values in [-1,1]",
                    ));
                }
                self.hooks.input(action).map(|()| json!({"queued": true}))
            }
            Command::Scene { query } => self.hooks.scene(query),
            Command::Capture { kind } => self.hooks.capture(&kind),
        }
    }
}
