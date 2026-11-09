//! Development-only resource watching for native hosts (debug builds only; release
//! hosts use their embedded resources). A worker polls source files and publishes the
//! caller's validated snapshot, so UI threads never touch the filesystem and only
//! receive complete results.
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    time::{Duration, SystemTime},
};

/// Dropping this handle stops the worker after its current poll.
pub type Stop = Sender<()>;

/// Start a worker named `name` that lists `files` every 250 ms. On the first poll and
/// whenever a file's size or modification time changes it sends `read()`, then calls
/// `wake` so an idle event loop can apply it. A failed read (an invalid or half-written
/// save) arrives as `Err`; callers keep their last good snapshot and report it.
pub fn watch<T: Send + 'static>(
    name: &str,
    files: impl Fn() -> Vec<PathBuf> + Send + 'static,
    read: impl Fn() -> Result<T, String> + Send + 'static,
    wake: impl Fn() + Send + 'static,
) -> std::io::Result<(Receiver<Result<T, String>>, Stop)> {
    let (tx, updates) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut fingerprint = Vec::new();
            loop {
                let next = fingerprint_of(files());
                if next != fingerprint {
                    fingerprint = next;
                    if tx.send(read()).is_err() {
                        break;
                    }
                    wake();
                }
                match stopped.recv_timeout(Duration::from_millis(250)) {
                    Err(RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
        })?;
    Ok((updates, stop))
}

fn fingerprint_of(mut paths: Vec<PathBuf>) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let metadata = path.metadata().ok();
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());
            let len = metadata.map_or(0, |m| m.len());
            (path, modified, len)
        })
        .collect()
}
