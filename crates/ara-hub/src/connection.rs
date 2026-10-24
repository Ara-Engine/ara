//! The Hub service connection, on its own thread so the window never blocks on an
//! RPC (`hub.open` can take seconds). It auto-starts the service, subscribes before
//! the model's first reads, forwards `hub.changed`, and reconnects after a loss.
use crate::model::Call;
use ara_diag::codes;
use ara_proto::{Endpoint, RpcError};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

/// What the connection reports to the view-model.
#[derive(Debug)]
pub enum Event {
    /// Connected and subscribed: every view must refresh (no replay is promised).
    Connected,
    /// The service is unreachable; the worker keeps retrying. `log` is `hub-start.log`.
    Offline {
        /// Why, for people.
        message: String,
        /// The service startup log.
        log: PathBuf,
    },
    /// The result of one [`Call`].
    Reply {
        /// The call, as sent.
        call: Call,
        /// Result or standard RpcError diagnostics.
        result: Result<Value, RpcError>,
    },
    /// A `hub.changed` notification.
    Changed {
        /// `registered`, `opened`, `activity`, `engine`, …
        kind: String,
        /// Affected project ID, if any.
        project: Option<String>,
    },
}

/// Handle to the worker; dropping it stops the worker after its current call.
pub struct Connection {
    calls: Sender<Call>,
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

const HEARTBEAT: Duration = Duration::from_secs(2);

impl Connection {
    /// Connect to the user Hub in `directory`, starting `executable hub serve` if needed.
    pub fn start(directory: PathBuf, executable: PathBuf) -> Self {
        let (calls, inbox) = mpsc::channel();
        let (outbox, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let worker = thread::Builder::new()
            .name("ara-hub-rpc".into())
            .spawn(move || worker(&directory, &executable, &inbox, &outbox, &flag))
            .map_or_else(
                |error| {
                    eprintln!("ara-hub: cannot start the connection thread: {error}");
                    None
                },
                Some,
            );
        Self {
            calls,
            events,
            stop,
            worker,
        }
    }
    /// Stop and join the reconnecting worker before a test shuts its service down.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
    /// Queue calls in order.
    pub fn send(&self, calls: impl IntoIterator<Item = Call>) {
        for call in calls {
            let _ = self.calls.send(call);
        }
    }
    /// Events that arrived since the last poll.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
    /// Wait for the next event (tests and capture runs).
    pub fn wait(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.stop();
    }
}

fn unreachable(result: &Result<Value, RpcError>) -> bool {
    result.as_ref().is_err_and(|error| {
        error
            .diagnostics()
            .iter()
            .any(|d| d.code == codes::DAEMON_UNREACHABLE)
    })
}

fn worker(
    directory: &Path,
    executable: &Path,
    inbox: &Receiver<Call>,
    outbox: &Sender<Event>,
    stop: &AtomicBool,
) {
    let log = directory.join("hub-start.log");
    let mut backoff = Duration::from_millis(250);
    while !stop.load(Ordering::Relaxed) {
        let connected = if executable.is_file() {
            ara_hub_service::connect_or_start(directory, executable).and_then(|mut client| {
                client.call("hub.subscribe", json!({}))?;
                Ok(client)
            })
        } else {
            Err(ara_diag::Diagnostic::error(
                codes::DAEMON_UNREACHABLE,
                format!(
                    "The Ara CLI was not found at `{}`; reinstall Ara or build `ara`",
                    executable.display()
                ),
            )
            .into())
        };
        let mut client = match connected {
            Ok(client) => client,
            Err(error) => {
                let message = error
                    .diagnostics()
                    .first()
                    .map_or_else(|| error.message.clone(), |d| d.message.clone());
                if outbox
                    .send(Event::Offline {
                        message,
                        log: log.clone(),
                    })
                    .is_err()
                {
                    return;
                }
                let until = Instant::now() + backoff;
                while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(25));
                }
                backoff = (backoff * 2).min(Duration::from_secs(4));
                continue;
            }
        };
        backoff = Duration::from_millis(250);
        if outbox.send(Event::Connected).is_err() {
            return;
        }
        let mut beat = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let mut lost = false;
            match inbox.recv_timeout(Duration::from_millis(15)) {
                Ok(call) => {
                    let result = client.call(call.method(), call.params());
                    // A downstream project daemon can also report "unreachable";
                    // only a failed heartbeat means the Hub socket itself is gone.
                    lost = unreachable(&result)
                        && unreachable(&client.call("hub.subscribe", json!({})));
                    if outbox.send(Event::Reply { call, result }).is_err() {
                        return;
                    }
                    beat = Instant::now();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let mut notes = client.notifications();
            notes.extend(client.wait_notification(Duration::from_millis(1)));
            for note in notes {
                if note["method"] == "hub.changed" {
                    let params = &note["params"];
                    let event = Event::Changed {
                        kind: params["kind"].as_str().unwrap_or_default().to_owned(),
                        project: params["project"].as_str().map(str::to_owned),
                    };
                    if outbox.send(event).is_err() {
                        return;
                    }
                }
            }
            if !lost && beat.elapsed() >= HEARTBEAT {
                lost = unreachable(&client.call("hub.subscribe", json!({})));
                beat = Instant::now();
            }
            if lost {
                let message = "Lost the Hub service; reconnecting".to_owned();
                if outbox
                    .send(Event::Offline {
                        message,
                        log: log.clone(),
                    })
                    .is_err()
                {
                    return;
                }
                break;
            }
        }
    }
}
