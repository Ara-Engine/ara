//! Atomic, verified publication of expendable cache data; no per-file fsync.
use super::{Error, Result, detail_scope};
use std::{fs, io::Write, path::Path};

enum Existing {
    Same,
    Missing,
    Damaged,
}

fn inspect(path: &Path, bytes: &[u8], valid: &impl Fn(&[u8]) -> Result<bool>) -> Result<Existing> {
    let _part = detail_scope("cache.install.read", &path.display().to_string());
    match fs::read(path) {
        Ok(existing) if existing == bytes => Ok(Existing::Same),
        Ok(existing) if valid(&existing)? => Err(Error::Invalid(
            "immutable cache entry conflicts (nondeterministic cook)".into(),
        )),
        Ok(_) => Ok(Existing::Damaged),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Existing::Missing),
        Err(e) => Err(e.into()),
    }
}

/// Repairs damaged files, but never replaces a different verified action.
pub(super) fn bytes(
    path: &Path,
    bytes: &[u8],
    valid: impl Fn(&[u8]) -> Result<bool>,
) -> Result<()> {
    let label = path.display().to_string();
    let _trace = crate::trace::Scope::new("cache.install", &label).bytes(bytes.len());
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("cache path without parent".into()))?;
    for _ in 0..8 {
        let state = inspect(path, bytes, &valid)?;
        let mut repair = None;
        let replace = match state {
            Existing::Same => return Ok(()),
            Existing::Missing => false,
            Existing::Damaged => {
                let _part = detail_scope("cache.install.repair_lock", &label);
                let name = path
                    .file_name()
                    .ok_or_else(|| Error::Invalid("cache path without name".into()))?;
                // Only damaged entries take a cross-process OS lock. Its empty
                // file is reusable, and process exit releases the lock itself.
                let lock = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(parent.join(format!(".repair-{}", name.to_string_lossy())))?;
                lock.lock()?;
                // Recheck after waiting: a repair may already have won. Atomic
                // replacement never creates a gap for a no-clobber publisher.
                let current = inspect(path, bytes, &valid)?;
                repair = Some(lock);
                match current {
                    Existing::Same => return Ok(()),
                    Existing::Missing => false,
                    Existing::Damaged => true,
                }
            }
        };
        let result = publish(path, bytes, replace);
        drop(repair);
        match result {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Invalid(
        "cache entry changed repeatedly while repairing".into(),
    ))
}

fn publish(path: &Path, bytes: &[u8], replace: bool) -> std::io::Result<()> {
    let label = path.display().to_string();
    let parent = path.parent().expect("cache parent validated by caller");
    let mut temp = {
        let _part = detail_scope("cache.install.create", &label);
        tempfile::NamedTempFile::new_in(parent)?
    };
    {
        let _part = detail_scope("cache.install.write", &label);
        #[cfg(test)]
        if super::tests::pause_at("mid-object-write", path) {
            let middle = bytes.len() / 2;
            temp.write_all(&bytes[..middle])?;
            super::tests::pause(path);
            temp.write_all(&bytes[middle..])?;
        } else {
            temp.write_all(bytes)?;
        }
        #[cfg(not(test))]
        temp.write_all(bytes)?;
    }
    {
        let _part = detail_scope("cache.install.persist", &label);
        let result = if replace {
            temp.persist(path)
        } else {
            temp.persist_noclobber(path)
        };
        result.map_err(|e| e.error)?;
    }
    #[cfg(test)]
    if super::tests::pause_at("object-published", path)
        || super::tests::pause_at("action-published", path)
    {
        super::tests::pause(path);
    }
    Ok(())
}
