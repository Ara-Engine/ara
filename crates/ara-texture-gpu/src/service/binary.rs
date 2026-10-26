//! Shadow a worker and adjacent native DLLs so a live helper never locks build outputs.
use ara_asset::{Digest, Error, Result};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct Binary {
    pub digest: Digest,
    root: PathBuf,
    executable: OsString,
    files: Vec<(OsString, Vec<u8>, std::fs::Permissions)>,
}
impl Binary {
    pub(super) fn load(cache: &Path) -> Result<Self> {
        let root = std::env::var_os("ARA_IMPORT_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache.join("workers"));
        Self::from_source(&std::env::current_exe()?, &root)
    }
    fn from_source(source: &Path, cache: &Path) -> Result<Self> {
        let parent = source
            .parent()
            .ok_or_else(|| Error::Invalid("worker executable has no directory".into()))?;
        let executable = source
            .file_name()
            .ok_or_else(|| Error::Invalid("worker executable has no name".into()))?
            .to_owned();
        let mut paths = vec![source.to_owned()];
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if (name.ends_with(".dll")
                || name.ends_with(".dylib")
                || name.ends_with(".so")
                || name.contains(".so."))
                && entry.path().is_file()
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        let mut hash = blake3::Hasher::new_derive_key("Ara import worker runtime copy v1");
        let mut files = Vec::new();
        for path in paths {
            let name = path
                .file_name()
                .expect("enumerated worker file has a name")
                .to_owned();
            let bytes = std::fs::read(&path)?;
            let name_bytes = name.as_encoded_bytes();
            hash.update(&(name_bytes.len() as u64).to_le_bytes());
            hash.update(name_bytes);
            hash.update(&(bytes.len() as u64).to_le_bytes());
            hash.update(&bytes);
            files.push((name, bytes, std::fs::metadata(path)?.permissions()));
        }
        Ok(Self {
            digest: Digest(*hash.finalize().as_bytes()),
            root: cache.to_owned(),
            executable,
            files,
        })
    }
    pub(super) fn publish(&self) -> Result<PathBuf> {
        let directory = self.root.join(self.digest.to_string());
        std::fs::create_dir_all(&directory)?;
        for (name, bytes, permissions) in &self.files {
            let path = directory.join(name);
            if path.exists() {
                continue;
            }
            let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
            temporary.write_all(bytes)?;
            temporary.as_file().set_permissions(permissions.clone())?;
            if let Err(error) = temporary.persist_noclobber(path)
                && error.error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(Error::Io(error.error));
            }
        }
        Ok(directory.join(&self.executable))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_copies_are_content_addressed_and_never_retarget_existing_helpers() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-texture-gpu/binary-copies");
        std::fs::create_dir_all(&root)?;
        let scratch = tempfile::tempdir_in(root)?;
        let source = scratch.path().join("source");
        let cache = scratch.path().join("cache");
        std::fs::create_dir(&source)?;
        let exe = source.join("worker.exe");
        std::fs::write(&exe, b"old executable")?;
        std::fs::write(source.join("native.dll"), b"old DLL")?;
        let first = Binary::from_source(&exe, &cache)?;
        let path = first.publish()?;
        assert_ne!(path, exe);
        assert_eq!(std::fs::read(&path)?, b"old executable");
        std::fs::write(source.join("native.dll"), b"new DLL")?;
        let second = Binary::from_source(&exe, &cache)?;
        assert_ne!(first.digest, second.digest);
        let second_path = second.publish()?;
        assert_eq!(
            std::fs::read(path.parent().expect("snapshot parent").join("native.dll"))?,
            b"old DLL"
        );
        assert_eq!(
            std::fs::read(
                second_path
                    .parent()
                    .expect("snapshot parent")
                    .join("native.dll")
            )?,
            b"new DLL"
        );
        std::fs::write(&exe, b"new executable")?;
        let third = Binary::from_source(&exe, &cache)?;
        assert_ne!(second.digest, third.digest);
        assert_eq!(std::fs::read(third.publish()?)?, b"new executable");
        assert_eq!(std::fs::read(path)?, b"old executable");
        Ok(())
    }
}
