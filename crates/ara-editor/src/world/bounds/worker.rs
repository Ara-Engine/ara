use super::{Digest, LocalModel, PublishedSnapshot};
use ara_runtime::assets::AssetSource;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
pub(super) enum Message {
    Loaded {
        asset: String,
        digest: Digest,
        result: Result<LocalModel, String>,
    },
    Done,
}
struct Batch {
    source: Arc<PublishedSnapshot>,
    assets: Vec<(String, Digest)>,
}
pub(super) struct Worker {
    request: SyncSender<Batch>,
    pub results: Receiver<Message>,
    stop: Arc<AtomicBool>,
}
impl Worker {
    pub fn new() -> std::io::Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Batch>(1);
        let (results_send, results) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("editor-model-bounds".into())
            .spawn(move || {
                #[cfg(windows)]
                {
                    use windows::Win32::System::Threading::{
                        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
                    };
                    // SAFETY: This worker changes only its own scheduling priority through a pseudo-handle.
                    let _ = unsafe {
                        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL)
                    };
                }
                while let Ok(batch) = receive.recv() {
                    for (asset, digest) in batch.assets {
                        if stopping.load(Ordering::Relaxed) {
                            return;
                        }
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let (loaded, cached) = batch
                                .source
                                .load(&asset)
                                .map_err(|error| error.to_string())?
                                .ok_or_else(|| "Published generation is unavailable".to_owned())?;
                            if loaded != digest {
                                return Err(
                                    "Published generation changed during bounds preparation".into(),
                                );
                            }
                            ara_runtime::model::Model::decode(&asset, digest, &cached)
                                .map(LocalModel::read)
                                .map_err(|error| error.to_string())
                        }))
                        .unwrap_or_else(|panic| {
                            Err(format!(
                                "Bounds worker panicked: {}",
                                panic
                                    .downcast_ref::<String>()
                                    .map(String::as_str)
                                    .or_else(|| panic.downcast_ref::<&str>().copied())
                                    .unwrap_or("unknown payload")
                            ))
                        });
                        if results_send
                            .send(Message::Loaded {
                                asset,
                                digest,
                                result,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    if results_send.send(Message::Done).is_err() {
                        return;
                    }
                }
            })?;
        Ok(Self {
            request: send,
            results,
            stop,
        })
    }
    pub fn request(
        &self,
        source: &Arc<PublishedSnapshot>,
        assets: Vec<(String, Digest)>,
    ) -> Result<(), String> {
        self.request
            .try_send(Batch {
                source: Arc::clone(source),
                assets,
            })
            .map_err(|error| format!("Model bounds worker unavailable: {error}"))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
