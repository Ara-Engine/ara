//! An independent heartbeat records permanent stalls, including outside a monkey action.
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

enum Message {
    Progress { step: u64, action: Option<Value> },
    Stop,
}

pub(super) struct Watchdog {
    sender: Sender<Message>,
    stalls: Receiver<u64>,
    worker: Option<JoinHandle<()>>,
    count: u64,
}

impl Watchdog {
    pub fn new(output: PathBuf, timeout: Duration) -> std::io::Result<Self> {
        std::fs::write(
            output.join("monkey-watchdog.json"),
            ara_json::to_canonical_string(&json!({"stalled":false,"hangs":0})),
        )?;
        let (sender, messages) = mpsc::channel();
        let (detected, stalls) = mpsc::channel();
        let worker = thread::Builder::new().name("editor-monkey-watchdog".into()).spawn(move || {
            let mut last = Instant::now();
            let mut step = 0;
            let mut action = None;
            let mut count = 0;
            let mut stalled = false;
            loop {
                match messages.recv_timeout(timeout) {
                    Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(Message::Progress { step:next, action:next_action }) => {
                        step = next;
                        if next_action.is_some() { action = next_action; }
                        last = Instant::now();
                        if stalled {
                            write(&output, json!({"stalled":false,"recovered":true,"hangs":count,"step":step,"lastAction":action}));
                        }
                        stalled = false;
                    }
                    Err(RecvTimeoutError::Timeout) if !stalled => {
                        stalled = true;
                        count += 1;
                        let _ = detected.send(count);
                        write(&output, json!({"stalled":true,"hangs":count,"step":step,"lastAction":action,"secondsWithoutHeartbeat":last.elapsed().as_secs_f64()}));
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        })?;
        Ok(Self {
            sender,
            stalls,
            worker: Some(worker),
            count: 0,
        })
    }

    pub fn progress(&mut self, step: u64, action: Option<Value>) {
        let _ = self.sender.send(Message::Progress { step, action });
        for count in self.stalls.try_iter() {
            self.count = self.count.max(count);
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn write(path: &std::path::Path, report: Value) {
    let _ = std::fs::write(
        path.join("monkey-watchdog.json"),
        ara_json::to_canonical_string(&report),
    );
}
