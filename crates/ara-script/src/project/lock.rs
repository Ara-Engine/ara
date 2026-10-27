//! Serialize derived assembly/schema publication across project preparation workers.
use super::io;
use ara_diag::Diagnostic;
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::Path,
    time::{Duration, Instant},
};

pub(super) struct PublicationLock {
    _file: File,
}
impl PublicationLock {
    pub fn acquire(root: &Path, cancelled: impl Fn() -> bool) -> Result<Self, Diagnostic> {
        let state = root.join(".ara/scripts");
        std::fs::create_dir_all(&state).map_err(io)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join("publication.lock"))
            .map_err(io)?;
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => return Err(io(error)),
            }
            if cancelled() || Instant::now() >= deadline {
                return Err(Diagnostic::error(
                    crate::codes::COMPILE_SERVER,
                    "C# publication was cancelled or another compilation held its 90 second preparation budget",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
