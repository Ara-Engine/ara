//! Short filesystem sockets shared by the application and its isolated monitor.
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;

// Leave room for the terminator in macOS's 104-byte sun_path; this also fits
// Windows and Linux's 108-byte addresses. Count UTF-8 bytes, not characters.
const MAX_PATH_BYTES: usize = 103;

pub(super) fn checked_name(path: &Path) -> Result<&str> {
    let name = path.to_str().context("non-UTF8 crash socket path")?;
    ensure!(
        name.len() <= MAX_PATH_BYTES,
        "socket path too long: {} bytes (maximum {MAX_PATH_BYTES}): {}",
        name.len(),
        path.display()
    );
    ensure!(
        !name.contains('\0'),
        "crash socket path contains a NUL byte"
    );
    Ok(name)
}

/// Removes only our socket and, for Capture-created endpoints, its private directory.
/// The monitor also owns this cleanup so it runs after an application crashes.
pub(super) struct SocketPath {
    pub path: PathBuf,
    private_directory: bool,
}

impl SocketPath {
    pub fn for_monitor(path: PathBuf, private_directory: bool) -> Result<Self> {
        checked_name(&path)?;
        Ok(Self {
            path,
            private_directory,
        })
    }

    pub fn create() -> Result<Self> {
        // An explicit override is strict: isolated launches must never spill into
        // the real profile or a shared temporary directory when it is unusable.
        let bases = match std::env::var_os("ARA_CRASH_IPC_DIR") {
            Some(base) => vec![PathBuf::from(base)],
            None => default_bases(),
        };
        Self::in_bases(&bases)
    }

    fn in_bases(bases: &[PathBuf]) -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let pid = std::process::id();
        let mut failures = Vec::new();
        for base in bases {
            let result = (|| {
                let base = std::path::absolute(base)?;
                for attempt in 0..16 {
                    let hash = blake3::hash(format!("{pid}-{stamp}-{attempt}").as_bytes()).to_hex();
                    let directory = base.join(format!("ara-c-{pid}-{}", &hash[..8]));
                    let path = directory.join("s");
                    checked_name(&path)?;
                    std::fs::create_dir_all(&base)?;
                    match private_directory(&directory) {
                        Ok(()) => {
                            return Ok(Self {
                                path,
                                private_directory: true,
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                bail!("crash socket directory collisions")
            })();
            match result {
                Ok(socket) => return Ok(socket),
                Err(error) => failures.push(format!("{}: {error:#}", base.display())),
            }
        }
        bail!("no usable crash IPC directory: {}", failures.join("; "))
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if self.private_directory
            && let Some(parent) = self.path.parent()
        {
            // Never recursively delete: a stale endpoint cannot remove other files.
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn default_bases() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut bases = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            bases.push(PathBuf::from(local).join("Ara/ipc"));
        }
        bases.push(std::env::temp_dir());
        // A deep/non-ASCII profile can make both per-user paths too long. The
        // Windows temp root stays short; each child gets a protected owner DACL.
        if let Some(system) = std::env::var_os("SystemRoot") {
            bases.push(PathBuf::from(system).join("Temp"));
        }
        bases
    }
    #[cfg(not(windows))]
    vec![std::env::temp_dir(), PathBuf::from("/tmp")]
}

#[cfg(windows)]
use windows::private_directory;

#[cfg(not(windows))]
fn private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn length_guard_counts_bytes_and_reserves_the_terminator() {
        assert!(checked_name(Path::new(&"a".repeat(MAX_PATH_BYTES))).is_ok());
        let long = "a".repeat(MAX_PATH_BYTES + 1);
        assert!(
            checked_name(Path::new(&long))
                .unwrap_err()
                .to_string()
                .contains("socket path too long: 104 bytes (maximum 103)")
        );
        let unicode = "é".repeat(82);
        assert!(
            checked_name(Path::new(&unicode))
                .unwrap_err()
                .to_string()
                .contains("socket path too long: 164 bytes")
        );
    }

    #[test]
    fn long_base_falls_back_before_creating_any_directory() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target/crash-ipc");
        std::fs::create_dir_all(&root).unwrap();
        let root = std::path::absolute(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let long = dir.path().join("deep-profile-".repeat(14));
        let socket = SocketPath::in_bases(&[long.clone(), dir.path().to_owned()]).unwrap();
        assert!(!long.exists());
        assert!(checked_name(&socket.path).is_ok());
        assert_eq!(socket.path.parent().unwrap().parent(), Some(dir.path()));
        let server = minidumper::Server::with_name(minidumper::SocketName::path(
            checked_name(&socket.path).unwrap(),
        ))
        .unwrap();
        drop(server);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(socket.path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let private = socket.path.parent().unwrap().to_owned();
        let other = SocketPath::in_bases(&[dir.path().to_owned()]).unwrap();
        assert_ne!(socket.path, other.path);
        drop(socket);
        assert!(!private.exists());
    }
}
