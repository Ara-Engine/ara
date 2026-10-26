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
    /// Bootstrap verification finished off-thread; native integrations may use this CLI.
    Prepared {
        /// Immutable, verified service executable, outside the GUI installation.
        executable: PathBuf,
    },
    /// Connected and subscribed: every view must refresh (no replay is promised).
    Connected,
    /// An older service is retained because it is supervising open projects.
    ServiceUpdatePending,
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

struct Outbox {
    events: Sender<Event>,
    wake: Box<dyn Fn() + Send>,
}
impl Outbox {
    fn send(&self, event: Event) -> Option<()> {
        self.events.send(event).ok()?;
        (self.wake)();
        Some(())
    }
}

const HEARTBEAT: Duration = Duration::from_secs(2);

impl Connection {
    /// Connect to the user Hub in `directory`, starting `executable hub serve` if needed.
    pub fn start(directory: PathBuf, executable: PathBuf) -> Self {
        Self::start_with_wake(directory, executable, || {})
    }
    /// Connect off-thread and wake a native event loop when results arrive.
    pub fn start_with_wake(
        directory: PathBuf,
        executable: PathBuf,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        Self::spawn(directory, move || Ok(executable.clone()), wake, false)
    }
    /// Prepare an installed bootstrap and connect on the worker, in parallel with
    /// device/first-frame creation. Preparation errors use normal reconnect backoff.
    pub fn start_preparing_with_wake(
        directory: PathBuf,
        prepare: impl FnMut() -> Result<PathBuf, RpcError> + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        Self::spawn(directory, prepare, wake, true)
    }
    fn spawn(
        directory: PathBuf,
        prepare: impl FnMut() -> Result<PathBuf, RpcError> + Send + 'static,
        wake: impl Fn() + Send + 'static,
        announce_prepared: bool,
    ) -> Self {
        let (calls, inbox) = mpsc::channel();
        let (outbox, events) = mpsc::channel();
        let outbox = Outbox {
            events: outbox,
            wake: Box::new(wake),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let worker = thread::Builder::new()
            .name("ara-hub-rpc".into())
            .spawn(move || {
                worker(
                    &directory,
                    prepare,
                    announce_prepared,
                    &inbox,
                    &outbox,
                    &flag,
                )
            })
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
        // The GUI must not join an in-flight blocking RPC when its window closes.
        // Tests call stop() explicitly before stopping their owned service.
        self.stop.store(true, Ordering::Relaxed);
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
    mut prepare: impl FnMut() -> Result<PathBuf, RpcError>,
    announce_prepared: bool,
    inbox: &Receiver<Call>,
    outbox: &Outbox,
    stop: &AtomicBool,
) {
    let log = directory.join("hub-start.log");
    let mut backoff = Duration::from_millis(250);
    let mut executable = None;
    while !stop.load(Ordering::Relaxed) {
        let connected = (|| {
            if executable.is_none() {
                let path = prepare()?;
                if announce_prepared {
                    let _ = outbox.send(Event::Prepared {
                        executable: path.clone(),
                    });
                }
                executable = Some(path);
            }
            let executable = executable
                .as_deref()
                .expect("successful preparation supplies a path");
            if executable.is_file() {
                ara_hub_service::connect_checked(directory, executable).and_then(|mut service| {
                    service.client.call("hub.subscribe", json!({}))?;
                    Ok(service)
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
            }
        })();
        let service = match connected {
            Ok(service) => service,
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
                    .is_none()
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
        if outbox.send(Event::Connected).is_none() {
            return;
        }
        if service.update_pending && outbox.send(Event::ServiceUpdatePending).is_none() {
            return;
        }
        let mut client = service.client;
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
                    if outbox.send(Event::Reply { call, result }).is_none() {
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
                    if outbox.send(event).is_none() {
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
                    .is_none()
                {
                    return;
                }
                break;
            }
        }
    }
}
