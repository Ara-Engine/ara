//! Bounded evidence IO: slow checkpoint storage must not stall native input.
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

enum Message {
    Checkpoint(String),
    Final(String),
}

struct Completed {
    elapsed: Duration,
    result: io::Result<()>,
}

/// Keep one waiting checkpoint while the worker owns the previous write.
pub(super) struct Reports {
    sender: Option<SyncSender<Message>>,
    completed: Receiver<Completed>,
    worker: Option<JoinHandle<()>>,
    submitted: u64,
    coalesced: u64,
    written: u64,
    total_ms: f64,
    max_ms: f64,
    error: Option<String>,
}

impl Reports {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        Self::spawn(path, |path, text| std::fs::write(path, text))
    }

    fn spawn(
        path: PathBuf,
        mut write: impl FnMut(&Path, &str) -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, messages) = mpsc::sync_channel(1);
        let (done, completed) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("editor-monkey-reports".into())
            .spawn(move || {
                while let Ok(message) = messages.recv() {
                    let (text, final_report) = match message {
                        Message::Checkpoint(text) => (text, false),
                        Message::Final(text) => (text, true),
                    };
                    let at = Instant::now();
                    let result = write(&path, &text);
                    let _ = done.send(Completed {
                        elapsed: at.elapsed(),
                        result,
                    });
                    if final_report {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            completed,
            worker: Some(worker),
            submitted: 0,
            coalesced: 0,
            written: 0,
            total_ms: 0.,
            max_ms: 0.,
            error: None,
        })
    }

    /// Submit without waiting for storage; skipped intermediate snapshots are counted.
    pub fn checkpoint(&mut self, text: String) -> io::Result<()> {
        self.collect();
        self.check_error()?;
        match self.sender()?.try_send(Message::Checkpoint(text)) {
            Ok(()) => self.submitted += 1,
            Err(TrySendError::Full(_)) => self.coalesced += 1,
            Err(TrySendError::Disconnected(_)) => {
                return Err(io::Error::other("Monkey report worker disconnected"));
            }
        }
        Ok(())
    }

    /// After input stops, observe every prior write before constructing final acceptance.
    pub fn drain_checkpoints(&mut self) -> io::Result<()> {
        self.collect();
        while self.written < self.submitted {
            let done = self
                .completed
                .recv()
                .map_err(|_| io::Error::other("Monkey report worker disconnected"))?;
            self.record(done);
        }
        self.check_error()
    }

    /// After the frame loop, wait for the final authoritative snapshot in write order.
    pub fn finish(&mut self, text: String) -> io::Result<()> {
        self.sender()?
            .send(Message::Final(text))
            .map_err(|_| io::Error::other("Monkey report worker disconnected"))?;
        self.sender.take();
        self.join()?;
        self.collect();
        self.check_error()
    }

    /// Actual storage costs are separate from time spent submitting on the UI thread.
    pub fn report(&mut self) -> Value {
        self.collect();
        json!({"submittedCheckpoints":self.submitted,"coalescedCheckpoints":self.coalesced,
            "completedWrites":self.written,"totalWriteMs":self.total_ms,"maxWriteMs":self.max_ms,
            "error":self.error,"scope":"Completed background writes before this snapshot; the final write is awaited after the frame loop."})
    }

    fn sender(&self) -> io::Result<&SyncSender<Message>> {
        self.sender
            .as_ref()
            .ok_or_else(|| io::Error::other("Monkey report writer already finished"))
    }

    fn collect(&mut self) {
        while let Ok(done) = self.completed.try_recv() {
            self.record(done);
        }
    }

    fn record(&mut self, done: Completed) {
        let ms = done.elapsed.as_secs_f64() * 1000.;
        self.total_ms += ms;
        self.max_ms = self.max_ms.max(ms);
        self.written += 1;
        if let Err(error) = done.result {
            self.error.get_or_insert_with(|| error.to_string());
        }
    }

    fn check_error(&self) -> io::Result<()> {
        match &self.error {
            Some(error) => Err(io::Error::other(error.as_str())),
            None => Ok(()),
        }
    }

    fn join(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("Monkey report worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for Reports {
    fn drop(&mut self) {
        self.sender.take();
        let _ = self.join();
    }
}

#[cfg(test)]
mod tests;
