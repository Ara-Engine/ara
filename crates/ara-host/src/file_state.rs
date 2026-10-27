//! Shared content-addressed file history. Call this adapter on a file/job worker.
//! The kernel stores only immutable tokens; this module owns every byte and restore.
use ara_diag::{Diagnostic, codes};
use ara_doc::{
    Op,
    file_state::{FileKind, FileState, check_path},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn io(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(codes::IO, error.to_string())
}

/// Project-local immutable snapshots, shared by source and asset commands.
#[derive(Clone, Debug)]
pub struct FileHistory {
    root: PathBuf,
}

impl FileHistory {
    /// Open the project's `.ara/file-history` store. Construction performs no I/O.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, relative: &str) -> Result<PathBuf, Diagnostic> {
        check_path(relative)?;
        let mut path = self.root.clone();
        for part in relative.split('/') {
            path.push(part);
            if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(io(format!(
                    "file history refuses symbolic link `{relative}`"
                )));
            }
        }
        Ok(path)
    }

    fn blob(&self, state: &FileState) -> Result<PathBuf, Diagnostic> {
        state.validate()?;
        Ok(self.root.join(".ara/file-history").join(&state.hash))
    }

    /// Store exact bytes durably; equal bytes share one immutable token.
    pub fn put(&self, bytes: &[u8]) -> Result<FileState, Diagnostic> {
        self.put_reader(std::io::Cursor::new(bytes))
    }

    fn put_reader(&self, mut input: impl Read) -> Result<FileState, Diagnostic> {
        let dir = self.root.join(".ara/file-history");
        fs::create_dir_all(&dir).map_err(io)?;
        let temp = dir.join(format!(
            ".pending-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(io)?;
            let state = hash_reader(&mut input, Some(&mut file))?;
            file.sync_all().map_err(io)?;
            drop(file);
            let target = self.blob(&state)?;
            if !target.exists() {
                fs::rename(&temp, target).map_err(io)?;
            }
            Ok(state)
        })();
        let _ = fs::remove_file(temp);
        result
    }

    /// Observe an entry without copying bytes. Symlinks and escaping paths are refused.
    pub fn observe(&self, relative: &str) -> Result<Option<FileState>, Diagnostic> {
        let path = self.path(relative)?;
        match fs::metadata(&path) {
            Ok(m) if m.is_dir() => Ok(Some(FileState::directory())),
            Ok(m) if m.is_file() => Ok(Some(hash_reader(
                &mut fs::File::open(path).map_err(io)?,
                None,
            )?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
            _ => Err(io("only regular files and directories have snapshots")),
        }
    }

    /// Snapshot a file or an empty directory; absence is an ordinary state.
    pub fn snapshot(&self, relative: &str) -> Result<Option<FileState>, Diagnostic> {
        let path = self.path(relative)?;
        match fs::metadata(&path) {
            Ok(m) if m.is_file() => self.put_reader(fs::File::open(path).map_err(io)?).map(Some),
            Ok(m) if m.is_dir() => Ok(Some(FileState::directory())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
            _ => Err(io("only regular files and directories have snapshots")),
        }
    }

    /// Snapshot a subtree, including empty directories, in parent-first path order.
    pub fn snapshot_tree(&self, relative: &str) -> Result<Vec<(String, FileState)>, Diagnostic> {
        fn walk(
            store: &FileHistory,
            path: &str,
            out: &mut Vec<(String, FileState)>,
        ) -> Result<(), Diagnostic> {
            if let Some(state) = store.snapshot(path)? {
                let directory = state.kind == FileKind::Directory;
                out.push((path.to_owned(), state));
                if directory {
                    let mut children = fs::read_dir(store.path(path)?)
                        .map_err(io)?
                        .map(|e| e.map(|e| e.file_name()))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(io)?;
                    children.sort();
                    for child in children {
                        walk(
                            store,
                            &format!(
                                "{path}/{}",
                                child
                                    .to_str()
                                    .ok_or_else(|| io("file names must be UTF-8"))?
                            ),
                            out,
                        )?;
                    }
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(self, relative, &mut out)?;
        Ok(out)
    }

    /// Read and verify a regular-file snapshot before proposing source edits.
    pub fn read(&self, state: &FileState) -> Result<Vec<u8>, Diagnostic> {
        if state.kind != FileKind::File {
            return Err(io("a directory has no source bytes"));
        }
        let bytes = fs::read(self.blob(state)?).map_err(io)?;
        if FileState::bytes(&bytes) != *state {
            return Err(io("file history snapshot is corrupt"));
        }
        Ok(bytes)
    }

    /// Check an expected current state, returning ARA1501 without changing files on mismatch.
    pub fn preflight(&self, path: &str, expected: Option<&FileState>) -> Result<(), Diagnostic> {
        if self.observe(path)?.as_ref() != expected {
            return Err(Diagnostic::error(codes::STALE_BASE, format!("file `{path}` changed since its base snapshot; refresh before applying or undoing")).file(path));
        }
        Ok(())
    }

    /// Restore one immutable state. Directory removal is empty-only; never recursive.
    pub fn restore(&self, relative: &str, state: Option<&FileState>) -> Result<(), Diagnostic> {
        let path = self.path(relative)?;
        match state {
            Some(state) if state.kind == FileKind::Directory => {
                state.validate()?;
                if path.is_file() {
                    fs::remove_file(&path).map_err(io)?;
                }
                fs::create_dir_all(path).map_err(io)
            }
            Some(state) => {
                let source = self.blob(state)?;
                let mut input = fs::File::open(source).map_err(io)?;
                let parent = path.parent().ok_or_else(|| io("file path has no parent"))?;
                fs::create_dir_all(parent).map_err(io)?;
                let temp = parent.join(format!(
                    ".ara-restore-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                let result = (|| {
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temp)
                        .map_err(io)?;
                    let restored = hash_reader(&mut input, Some(&mut output))?;
                    if restored != *state {
                        return Err(io("file history snapshot is corrupt"));
                    }
                    output.sync_all().map_err(io)?;
                    drop(output);
                    if path.is_dir() {
                        fs::remove_dir(&path).map_err(io)?;
                    }
                    fs::rename(&temp, &path).map_err(io)
                })();
                let _ = fs::remove_file(temp);
                result
            }
            None => match fs::metadata(&path) {
                Ok(m) if m.is_dir() => fs::remove_dir(path).map_err(io),
                Ok(_) => fs::remove_file(path).map_err(io),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io(e)),
            },
        }
    }

    /// Preflight every transition before writing any; supports repeated paths in a batch.
    pub fn preflight_ops(&self, ops: &[Op]) -> Result<(), Diagnostic> {
        let mut staged = BTreeMap::new();
        for op in ops {
            if let Op::File {
                path, state, prev, ..
            } = op
            {
                let current = match staged.get(path) {
                    Some(s) => s,
                    None => {
                        self.preflight(path, prev.as_ref())?;
                        prev
                    }
                };
                if current != prev {
                    return Err(Diagnostic::error(
                        codes::STALE_BASE,
                        "inconsistent file transition sequence",
                    )
                    .file(path));
                }
                if let Some(s) = state {
                    s.validate()?;
                    if s.kind == FileKind::File && !self.blob(s)?.is_file() {
                        return Err(io(format!("missing file snapshot {}", s.hash)));
                    }
                }
                staged.insert(path.clone(), state.clone());
            }
        }
        Ok(())
    }

    /// Materialize a committed transaction on the file worker. A failed write attempts
    /// to restore preceding entries; the durable journal remains the recovery authority.
    pub fn materialize(&self, ops: &[Op]) -> Result<(), Diagnostic> {
        self.preflight_ops(ops)?;
        let mut done = Vec::new();
        for op in ops {
            if let Op::File {
                path, state, prev, ..
            } = op
            {
                if let Err(error) = self.restore(path, state.as_ref()) {
                    for (path, previous) in done.into_iter().rev() {
                        self.restore(path, previous)?;
                    }
                    return Err(error);
                }
                done.push((path.as_str(), prev.as_ref()));
            }
        }
        Ok(())
    }
}

fn hash_reader(
    input: &mut impl Read,
    mut copy: Option<&mut fs::File>,
) -> Result<FileState, Diagnostic> {
    let mut hash = Sha256::new();
    let mut len = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        len += n as u64;
        if let Some(output) = &mut copy {
            output.write_all(&buffer[..n]).map_err(io)?;
        }
    }
    Ok(FileState {
        hash: format!("{:x}", hash.finalize()),
        len,
        kind: FileKind::File,
    })
}
