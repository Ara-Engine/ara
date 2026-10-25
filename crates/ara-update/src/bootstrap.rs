//! Immutable, content-addressed service copies, independent of the Hub install.
use crate::{Error, IO, sha256_file, write_json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Stage the bundled service, then publish its directory by rename. Old builds
/// remain runnable until their clients finish; a same-version preview gets a new path.
pub fn prepare(directory: &Path, base: &Path, version: &str) -> Result<PathBuf, Error> {
    crate::version(version)?;
    let source = directory.join("bootstrap");
    let mut files = fs::read_dir(&source)?
        .map(|entry| {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err(Error::new(IO, "service bootstrap must contain only files"));
            }
            Ok(entry.path())
        })
        .collect::<Result<Vec<_>, Error>>()?;
    files.sort();
    let executable = format!("ara{}", std::env::consts::EXE_SUFFIX);
    let build = sha256_file(&source.join(&executable))?;
    let mut digest = Sha256::new();
    let mut fingerprints = Vec::new();
    for file in &files {
        // The generated marker is metadata, not part of the payload identity.
        if file.file_name().is_some_and(|name| name == "service.json") {
            continue;
        }
        digest.update(
            file.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
        digest.update([0]);
        let hash = sha256_file(file)?;
        digest.update(hash.as_bytes());
        fingerprints.push((file.file_name().unwrap_or_default().to_owned(), hash));
    }
    let content = hex::encode(digest.finalize());
    let base = base.join(version);
    fs::create_dir_all(&base)?;
    // Serialize concurrent Hubs without ever locking a running executable.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(base.join("bootstrap.lock"))?;
    lock.lock()?;
    let target = base.join(&content);
    if !target.exists() {
        let staged = base.join(format!(".stage-{}", std::process::id()));
        // Only our own incomplete staging directory can be left by an interrupted copy.
        if staged.exists() {
            fs::remove_dir_all(&staged)?;
        }
        fs::create_dir(&staged)?;
        let copied = (|| {
            for file in &files {
                let name = file
                    .file_name()
                    .ok_or_else(|| Error::new(IO, "bootstrap filename"))?;
                fs::copy(file, staged.join(name))?;
            }
            for (name, hash) in &fingerprints {
                if sha256_file(&staged.join(name))? != *hash {
                    return Err(Error::new(
                        IO,
                        "Hub bootstrap changed during staging; retry startup",
                    ));
                }
            }
            write_json(
                &staged.join("service.json"),
                &serde_json::json!({"version":version,"build":build,"content":content}),
            )?;
            fs::rename(&staged, &target)?;
            Ok(())
        })();
        if copied.is_err() {
            let _ = fs::remove_dir_all(&staged);
        }
        copied?;
    }
    for (name, hash) in &fingerprints {
        if sha256_file(&target.join(name))? != *hash {
            return Err(Error::new(
                IO,
                "Hub bootstrap copy is damaged; reinstall Ara Hub",
            ));
        }
    }
    Ok(target.join(executable))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn same_version_new_build_stages_without_overwriting_the_old_one() {
        let root = crate::testing::scratch("bootstrap");
        let hub = root.join("Hub");
        fs::create_dir_all(hub.join("bootstrap")).unwrap();
        let name = format!("ara{}", std::env::consts::EXE_SUFFIX);
        fs::write(hub.join("bootstrap").join(&name), b"preview 8").unwrap();
        let base = root.join("Services");
        fs::create_dir_all(base.join("0.0.1-alpha")).unwrap();
        fs::write(
            base.join("0.0.1-alpha").join(&name),
            b"legacy stale service",
        )
        .unwrap();
        let old = prepare(&hub, &base, "0.0.1-alpha").unwrap();
        let held = fs::File::open(&old).unwrap();
        fs::write(hub.join("bootstrap").join(&name), b"preview 9").unwrap();
        let new = prepare(&hub, &base, "0.0.1-alpha").unwrap();
        assert_ne!(old, new);
        assert_eq!(fs::read(&old).unwrap(), b"preview 8");
        assert_eq!(fs::read(&new).unwrap(), b"preview 9");
        assert_eq!(new, prepare(&hub, &base, "0.0.1-alpha").unwrap());
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(new.with_file_name("service.json")).unwrap()).unwrap();
        assert_eq!(marker["build"], sha256_file(&new).unwrap());
        assert!(
            !fs::read_dir(new.parent().unwrap().parent().unwrap())
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".stage-"))
        );
        drop(held);
    }
}
