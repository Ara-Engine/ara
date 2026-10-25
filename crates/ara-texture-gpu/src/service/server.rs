//! Bounded reusable GPU slots, with host leases separate from disposable clients.
use super::wire::{self, Descriptor, Reply};
use ara_asset::{BlockEncoder, Cancellation, EncoderEndpoint, Error, Result};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct Pool {
    encoders: Vec<Box<dyn BlockEncoder>>,
    idle: Mutex<Vec<usize>>,
    available: Condvar,
    leases: AtomicUsize,
    used: AtomicBool,
    connections: AtomicUsize,
    clients: AtomicUsize,
    initialized: AtomicUsize,
    stop: AtomicBool,
    cancel: Cancellation,
    io_timeout: Duration,
}
impl Pool {
    pub(super) fn initialized(&self) -> usize {
        self.initialized.load(Ordering::Acquire)
    }
    pub(super) fn new(encoders: Vec<Box<dyn BlockEncoder>>) -> Self {
        Self {
            idle: Mutex::new((0..encoders.len()).rev().collect()),
            encoders,
            available: Condvar::new(),
            leases: AtomicUsize::new(0),
            used: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
            clients: AtomicUsize::new(0),
            initialized: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            cancel: Cancellation::default(),
            io_timeout: Duration::from_secs(30),
        }
    }
    #[cfg(test)]
    pub(super) fn with_io_timeout(mut self, timeout: Duration) -> Self {
        self.io_timeout = timeout;
        self
    }
    fn acquire(&self) -> Result<Slot<'_>> {
        let mut idle = self
            .idle
            .lock()
            .map_err(|_| Error::EncoderUnavailable("codec pool poisoned".into()))?;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if let Some(index) = idle.pop() {
                return Ok(Slot { pool: self, index });
            }
            idle = self
                .available
                .wait_timeout(idle, Duration::from_millis(100))
                .map_err(|_| Error::EncoderUnavailable("codec pool poisoned".into()))?
                .0;
        }
    }
}
struct Slot<'a> {
    pool: &'a Pool,
    index: usize,
}
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        if let Ok(mut idle) = self.pool.idle.lock() {
            idle.push(self.index);
            self.pool.available.notify_one();
        }
    }
}
struct Connection<'a>(&'a Pool);
impl Drop for Connection<'_> {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Lease<'a>(&'a Pool);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.leases.fetch_sub(1, Ordering::AcqRel);
    }
}
struct ClientLife<'a>(&'a Pool);
impl Drop for ClientLife<'_> {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn serve(
    listener: TcpListener,
    descriptor: &Descriptor,
    pool: Arc<Pool>,
    grace: Duration,
) -> Result<()> {
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    let mut idle_since = None;
    thread::scope(|scope| -> Result<()> {
        loop {
            let leased =
                pool.leases.load(Ordering::Acquire) > 0 || pool.clients.load(Ordering::Acquire) > 0;
            if leased {
                idle_since = None;
            } else {
                let since = idle_since.get_or_insert_with(Instant::now);
                let expired = if pool.used.load(Ordering::Acquire) {
                    since.elapsed() > grace
                } else {
                    started.elapsed() > Duration::from_secs(30)
                };
                if expired {
                    break;
                }
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if pool.connections.fetch_add(1, Ordering::AcqRel) >= 16 {
                        pool.connections.fetch_sub(1, Ordering::AcqRel);
                        drop(stream);
                        continue;
                    }
                    let pool = &pool;
                    let descriptor = descriptor.clone();
                    scope.spawn(move || {
                        let _connection = Connection(pool);
                        let _ = handle(stream, &descriptor, pool);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => return Err(e.into()),
            }
        }
        pool.stop.store(true, Ordering::Release);
        pool.cancel.cancel();
        pool.available.notify_all();
        Ok(())
    })
}

fn handle(mut stream: TcpStream, descriptor: &Descriptor, pool: &Pool) -> Result<()> {
    // Windows accepted sockets inherit the listener's nonblocking mode. Only
    // accept is polled; each bounded client handler uses blocking framed I/O.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(pool.io_timeout))?;
    stream.set_write_timeout(Some(pool.io_timeout))?;
    let mut hello = [0u8; 41];
    stream.read_exact(&mut hello)?;
    if &hello[..8] != b"ARAENC01" || hello[8..40] != descriptor.endpoint.token || hello[40] > 1 {
        stream.write_all(&[1])?;
        return Ok(());
    }
    if pool.stop.load(Ordering::Acquire) {
        stream.write_all(&[1])?;
        return Ok(());
    }
    pool.used.store(true, Ordering::Release);
    if hello[40] == 0 {
        pool.leases.fetch_add(1, Ordering::AcqRel);
        let _lease = Lease(pool);
        stream.write_all(&[0])?;
        stream.set_read_timeout(None)?;
        let _ = stream.read(&mut [0]);
        return Ok(());
    }
    pool.clients.fetch_add(1, Ordering::AcqRel);
    let _client = ClientLife(pool);
    stream.write_all(&[0])?;
    wire::write_json(&mut stream, &descriptor.identity)?;
    loop {
        // A worker can spend a long time in CPU preparation or filesystem I/O
        // between encodes. Its host owns the import deadline and cancellation.
        stream.set_read_timeout(None)?;
        let mut op = [0];
        match stream.read_exact(&mut op) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        if pool.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        stream.set_read_timeout(Some(pool.io_timeout))?;
        let input = match op[0] {
            0 => None,
            1 => {
                let mut header = [0; 9];
                stream.read_exact(&mut header)?;
                let format = wire::format(u32::from_le_bytes(
                    header[..4].try_into().expect("four format bytes"),
                ))?;
                let preset = wire::preset(header[4])?;
                let count =
                    u32::from_le_bytes(header[5..].try_into().expect("four count bytes")) as usize;
                wire::validate_count(format, count)?;
                Some((format, preset, wire::read_words(&mut stream, count)?))
            }
            _ => return Err(Error::Invalid("unknown codec service operation".into())),
        };
        let queued = Instant::now();
        let slot = pool.acquire()?;
        let queue_ms = queued.elapsed().as_secs_f64() * 1000.0;
        let encoder = &pool.encoders[slot.index];
        let before = encoder.metrics();
        let result = match input {
            Some((format, preset, words)) => encoder.encode(format, preset, &words, &pool.cancel),
            None => encoder.prepare().map(|_| Vec::new()),
        };
        let after = encoder.metrics();
        if !before.contains_key("initialize.wall_ms") && after.contains_key("initialize.wall_ms") {
            pool.initialized.fetch_add(1, Ordering::AcqRel);
            ara_asset::record_import_trace(
                "texture.service.initialize",
                "gpu-codec",
                serde_json::json!({"slot":slot.index,"elapsedMs":after["initialize.wall_ms"]}),
            );
        }
        let mut metrics = after
            .into_iter()
            .map(|(key, value)| {
                let delta = value - before.get(&key).copied().unwrap_or(0.0);
                (key, delta)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        metrics.insert("pool.queue.wall_ms".into(), queue_ms);
        metrics.insert("service.pid".into(), f64::from(std::process::id()));
        metrics.insert(
            "service.devices_created".into(),
            pool.initialized.load(Ordering::Acquire) as f64,
        );
        drop(slot);
        let (error, bytes) = match result {
            Ok(bytes) => (None, bytes),
            Err(error) => (Some(error.to_string()), Vec::new()),
        };
        wire::write_json(
            &mut stream,
            &Reply {
                error,
                bytes: bytes.len(),
                metrics,
            },
        )?;
        stream.write_all(&bytes)?;
        stream.flush()?;
    }
}

pub(super) fn descriptor(listener: &TcpListener, identity: String) -> Result<Descriptor> {
    let mut token = [0; 32];
    getrandom::fill(&mut token).map_err(|e| Error::EncoderUnavailable(e.to_string()))?;
    Ok(Descriptor {
        endpoint: EncoderEndpoint {
            address: listener.local_addr()?,
            token,
        },
        identity,
        pid: std::process::id(),
    })
}
