//! Bound staged binaries without deleting an active or queued last-good generation.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub(super) fn pin(base: &Path, executable: &Path) -> Result<Option<File>> {
    let parent = executable.parent().context("player directory")?;
    if !parent.starts_with(base.join("players")) {
        return Ok(None);
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".lease"))?;
    file.lock_shared()?;
    ensure!(executable.is_file(), "player generation disappeared");
    Ok(Some(file))
}
pub(super) fn prune(base: &Path) -> Result<usize> {
    let root = base.join("players").canonicalize()?;
    let mut directories = fs::read_dir(&root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|ty| ty.is_dir() && !ty.is_symlink())
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|entry| {
        (
            entry.metadata().and_then(|m| m.modified()).ok(),
            entry.file_name(),
        )
    });
    let take = directories.len().saturating_sub(4);
    let mut removed = 0;
    for entry in directories.into_iter().take(take) {
        let directory = entry.path().canonicalize()?;
        ensure!(
            directory.parent() == Some(root.as_path()),
            "generation cleanup escaped its cache"
        );
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(".lease"))?;
        if lease.try_lock().is_err() {
            continue;
        }
        match fs::remove_dir_all(&directory) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(removed)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_and_running_generations_survive_retention() -> Result<()> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/lab-tests")
            .join(super::super::session::id());
        for i in 0..6 {
            let directory = base.join("players").join(i.to_string());
            fs::create_dir_all(&directory)?;
            fs::write(directory.join("player.exe"), b"fixture")?;
            fs::write(directory.join(".lease"), b"")?;
        }
        let first = base.join("players/0/player.exe");
        let live = pin(&base, &first)?;
        assert_eq!(prune(&base)?, 1);
        assert!(first.is_file());
        drop(live);
        assert_eq!(prune(&base)?, 1);
        assert!(!first.exists());
        Ok(())
    }
}
