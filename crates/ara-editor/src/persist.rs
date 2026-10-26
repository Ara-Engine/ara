//! Files the editor saves off its UI thread (the window layout, preferences, the last
//! camera per scene). A change hands the newest value to a writer thread; the thread
//! writes it (a temporary file, then a rename) and skips any value a newer one has
//! already replaced. Dropping the writer flushes. A disk stall (an antivirus scan, an
//! import saturating the disk) then never stalls a frame: a theme switch during a cold
//! import once took 2.9 s writing the preferences file.
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// A writer thread for one file; `T` is what each save hands over.
pub(crate) struct BackgroundWriter<T: Send + 'static> {
    tx: Option<Sender<T>>,
    failures: Receiver<String>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> BackgroundWriter<T> {
    /// Start a writer named `name` (its thread name); `write` runs on that thread with
    /// the newest value and returns a message people can read when it fails.
    pub fn new(name: &str, write: impl Fn(T) -> Result<(), String> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<T>();
        let (failed, failures) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while let Ok(mut value) = rx.recv() {
                    // Only the newest value matters.
                    if let Some(newest) = rx.try_iter().last() {
                        value = newest;
                    }
                    if let Err(error) = write(value) {
                        let _ = failed.send(error);
                    }
                }
            })
            .ok();
        Self {
            tx: Some(tx),
            failures,
            thread,
        }
    }
    /// Hand over a new value to write.
    pub fn save(&self, value: T) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(value);
        }
    }
    /// Write failures since the last call.
    pub fn take_failures(&self) -> Vec<String> {
        self.failures.try_iter().collect()
    }
}

impl<T: Send + 'static> Drop for BackgroundWriter<T> {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Write `text` to `path` through a temporary file, so a reader never sees half of
/// it; creates the parent folder.
pub(crate) fn write_text(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The newest value wins and is on disk once the writer is dropped, however
    /// fast the saves came; failures come back as messages.
    #[test]
    fn the_newest_value_is_written_and_failures_are_reported() {
        let dir = std::env::temp_dir().join(format!("ara-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("value.txt");
        let target = path.clone();
        let writer = BackgroundWriter::new("persist-test", move |text: String| {
            write_text(&target, &text).map_err(|e| e.to_string())
        });
        for n in 0..50 {
            writer.save(format!("value {n}"));
        }
        drop(writer);
        assert_eq!(
            std::fs::read_to_string(&path).ok().as_deref(),
            Some("value 49")
        );
        let failing = BackgroundWriter::new("persist-test-fail", |_: ()| Err("disk full".into()));
        failing.save(());
        let mut failures = Vec::new();
        for _ in 0..400 {
            failures = failing.take_failures();
            if !failures.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(failures, ["disk full"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
