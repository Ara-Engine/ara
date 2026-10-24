//! Coalesced preference writes run outside the UI/input thread.
use crate::model::Prefs;
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread,
};

pub(super) struct Writer {
    send: Option<Sender<Prefs>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Writer {
    pub(super) fn new(config: PathBuf) -> io::Result<Self> {
        let (send, pending) = mpsc::channel::<Prefs>();
        let worker = thread::Builder::new()
            .name("hub-prefs".into())
            .spawn(move || {
                while let Ok(mut prefs) = pending.recv() {
                    for latest in pending.try_iter() {
                        prefs = latest;
                    }
                    if let Ok(value) = serde_json::to_value(prefs) {
                        let _ = std::fs::create_dir_all(&config);
                        let _ = std::fs::write(
                            config.join("hub-ui.json"),
                            ara_json::to_canonical_string(&value),
                        );
                    }
                }
            })?;
        Ok(Self {
            send: Some(send),
            worker: Some(worker),
        })
    }
    pub(super) fn save(&self, prefs: Prefs) {
        if let Some(send) = &self.send {
            let _ = send.send(prefs);
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.send.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
