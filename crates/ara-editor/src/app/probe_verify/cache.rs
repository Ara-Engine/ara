//! Native proof observes post-receipt cache publication without editor-thread I/O.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

const PUBLICATION_WAIT: Duration = Duration::from_secs(30);
const RETRY: Duration = Duration::from_millis(25);

pub(super) struct Publication {
    result: Receiver<Result<Value, String>>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
}
impl Publication {
    pub(super) fn start(root: PathBuf, logical: String) -> ara_ui::Result<Self> {
        Self::with_wait(root, logical, PUBLICATION_WAIT)
    }
    fn with_wait(root: PathBuf, logical: String, limit: Duration) -> ara_ui::Result<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let started = Instant::now();
        std::thread::Builder::new().name("ara-editor-probe-cache-proof".into()).spawn(move || {
            let path = root.join(&logical);
            let observed = loop {
                if stop.load(Ordering::Relaxed) { return; }
                match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {
                        break Ok(json!({"path":logical,"absolutePath":path,"bytes":metadata.len(),
                            "exists":true,"observedAfterReceipt":true,"worker":"ara-editor-probe-cache-proof",
                            "publicationWaitSeconds":started.elapsed().as_secs_f64()}));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => break Err(format!("Probe image is published, but cache metadata could not be read: {error}")),
                }
                if started.elapsed() >= limit {
                    break Err(format!("Probe image was published and hashed, but cache `{logical}` was not published as a nonempty file within {} seconds", limit.as_secs_f64()));
                }
                std::thread::sleep(RETRY.min(limit.saturating_sub(started.elapsed())));
            };
            let _ = sender.send(observed);
        })?;
        Ok(Self {
            result,
            cancelled,
            started,
        })
    }
    /// Mailbox-only polling; a missing file is pending until the worker's deadline.
    pub(super) fn poll(&self) -> ara_ui::Result<Option<Value>> {
        match self.result.try_recv() {
            Ok(Ok(value)) => Ok(Some(value)),
            Ok(Err(error)) => Err(error.into()),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(format!(
                "Probe cache observation worker stopped after {:.3}s without a result",
                self.started.elapsed().as_secs_f64()
            )
            .into()),
        }
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_can_precede_the_actual_cache_file_without_synchronous_failure() -> ara_ui::Result<()>
    {
        let root = std::path::absolute(crate::test_target())?
            .join("probe-cache-publication")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root)?;
        let observation =
            Publication::with_wait(root.clone(), "later.capture".into(), Duration::from_secs(2))?;
        assert!(observation.poll()?.is_none());
        std::fs::write(root.join("later.capture"), [])?;
        assert!(
            observation.poll()?.is_none(),
            "an empty cache file is not publication proof"
        );
        std::fs::write(
            root.join("later.capture"),
            b"cache metadata fixture; not GPU evidence",
        )?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(value) = observation.poll()? {
                assert_eq!(value["exists"], true);
                assert_eq!(value["observedAfterReceipt"], true);
                break;
            }
            assert!(Instant::now() < deadline, "cache observer did not finish");
            std::thread::yield_now();
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn missing_cache_publication_has_a_bounded_failure() -> ara_ui::Result<()> {
        let root = std::path::absolute(crate::test_target())?
            .join("absent-native-probe-cache")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
        let observation =
            Publication::with_wait(root, "missing.capture".into(), Duration::from_millis(20))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match observation.poll() {
                Err(error) => {
                    assert!(error.to_string().contains("published and hashed"));
                    break;
                }
                Ok(Some(_)) => return Err("Missing cache unexpectedly existed".into()),
                Ok(None) => {}
            }
            assert!(
                Instant::now() < deadline,
                "missing cache observer did not fail"
            );
            std::thread::yield_now();
        }
        Ok(())
    }
}
