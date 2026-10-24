//! A single transport owner keeps liveness independent of simulation and uploads.
use super::{Link, LinkError, Message};
use serde::Serialize;
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_millis(250);
const QUEUE: usize = 1024;
enum Outgoing {
    Notify(String, Value),
    Respond(Value, Result<Value, Value>),
}

/// Player-side connection whose owned IO thread sends heartbeats even while the
/// simulation or GPU upload thread is busy. Queues are bounded; no socket is shared
/// or protected by a frame-thread lock. Call after the synchronous hello handshake.
pub struct Pump {
    outgoing: SyncSender<Outgoing>,
    incoming: Receiver<Result<Message, LinkError>>,
    stop: Arc<AtomicBool>,
    heartbeat: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}
impl Pump {
    /// Transfers the connected socket to a dedicated IO/liveness thread.
    pub fn new(mut link: Link) -> std::io::Result<Self> {
        let (outgoing, requests) = mpsc::sync_channel(QUEUE);
        let (messages, incoming) = mpsc::sync_channel(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let heartbeat = Arc::new(AtomicBool::new(true));
        let worker_stop = Arc::clone(&stop);
        let worker_heartbeat = Arc::clone(&heartbeat);
        let task = thread::Builder::new()
            .name("ara-play-heartbeat".into())
            .spawn(move || {
                let mut last = Instant::now() - INTERVAL;
                let mut pending = None;
                loop {
                    let sent = (|| -> Result<(), LinkError> {
                        // A bounded batch prevents producer traffic from starving the
                        // liveness timer or incoming commands.
                        for _ in 0..QUEUE {
                            match requests.try_recv() {
                                Ok(Outgoing::Notify(method, params)) => {
                                    link.notify(&method, params)?
                                }
                                Ok(Outgoing::Respond(id, result)) => link.respond(id, result)?,
                                Err(_) => break,
                            }
                        }
                        if worker_heartbeat.load(Ordering::Acquire) && last.elapsed() >= INTERVAL {
                            link.notify(crate::HEARTBEAT, ())?;
                            last = Instant::now();
                        }
                        link.flush()
                    })();
                    if let Err(error) = sent {
                        let _ = messages.try_send(Err(error));
                        break;
                    }
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    if pending.is_none() {
                        pending = match link.poll() {
                            Ok(Some(message)) => Some(Ok(message)),
                            Ok(None) => None,
                            Err(error) => Some(Err(error)),
                        };
                    }
                    if let Some(message) = pending.take() {
                        let failed = message.is_err();
                        match messages.try_send(message) {
                            Ok(()) if failed => break,
                            Ok(()) => {}
                            Err(mpsc::TrySendError::Full(message)) => pending = Some(message),
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        }
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            })?;
        Ok(Self {
            outgoing,
            incoming,
            stop,
            heartbeat,
            task: Some(task),
        })
    }
    /// Receive one command without waiting on IO or simulation work.
    pub fn poll(&mut self) -> Result<Option<Message>, LinkError> {
        match self.incoming.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(LinkError::Closed),
        }
    }
    /// Idle without spinning; heartbeats continue on the IO thread.
    pub fn wait(&mut self, timeout: Duration) -> Result<Option<Message>, LinkError> {
        match self.incoming.recv_timeout(timeout) {
            Ok(result) => result.map(Some),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(LinkError::Closed),
        }
    }
    fn send(&self, command: Outgoing) -> Result<(), LinkError> {
        self.outgoing
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    LinkError::Io("player transport queue is full".into())
                }
                mpsc::TrySendError::Disconnected(_) => LinkError::Closed,
            })
    }
    /// Queue a notification for the sole socket owner.
    pub fn notify(&mut self, method: &str, params: impl Serialize) -> Result<(), LinkError> {
        let params = serde_json::to_value(params).map_err(|e| LinkError::Io(e.to_string()))?;
        self.send(Outgoing::Notify(method.to_owned(), params))
    }
    /// Queue a response, preserving the caller's request identifier.
    pub fn respond(&mut self, id: Value, result: Result<Value, Value>) -> Result<(), LinkError> {
        self.send(Outgoing::Respond(id, result))
    }
    /// Writes are driven continuously by the IO thread; never wait for it here.
    pub fn flush(&mut self) -> Result<(), LinkError> {
        if self.task.as_ref().is_some_and(|task| task.is_finished()) {
            Err(LinkError::Closed)
        } else {
            Ok(())
        }
    }
    /// Fault-injection only: model a genuinely silent process while its main thread
    /// hangs. Ordinary heavy work must leave the independent heartbeat enabled.
    pub fn suspend_heartbeat(&self) {
        self.heartbeat.store(false, Ordering::Release);
    }
}
impl Drop for Pump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_consumer_keeps_heartbeating_and_queued_commands_are_retained()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let owner = thread::spawn(move || -> Result<_, LinkError> {
            let (stream, _) = listener
                .accept()
                .map_err(|e| LinkError::Io(e.to_string()))?;
            let mut link = Link::accept(stream)?;
            link.notify("held-command", ())?;
            let began = Instant::now();
            let mut count = 0;
            while began.elapsed() < Duration::from_millis(1600) {
                if let Some(Message::Notification { method, .. }) = link.wait(INTERVAL)?
                    && method == crate::HEARTBEAT
                {
                    count += 1;
                }
            }
            Ok(count)
        });
        let mut pump = Pump::new(Link::connect(port)?)?;
        // The simulation does no polling or status production for longer than
        // the former watchdog. Only the owned IO thread can keep it alive.
        thread::sleep(Duration::from_millis(1800));
        assert!(owner.join().map_err(|_| "owner panicked")?? >= 5);
        assert!(
            matches!(pump.poll()?, Some(Message::Notification { method, .. }) if method == "held-command")
        );
        Ok(())
    }
}
