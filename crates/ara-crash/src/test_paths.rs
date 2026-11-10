//! Crash fixtures must not inherit the worktree's path length.
use std::path::PathBuf;

/// Allocate a private test directory with room for the longest socket suffix.
pub(crate) fn directory() -> tempfile::TempDir {
    #[cfg(not(windows))]
    let bases = vec![PathBuf::from("/tmp")];
    #[cfg(windows)]
    let bases = std::iter::once(std::env::temp_dir())
        .chain(std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("Temp")))
        .collect::<Vec<_>>();
    for base in bases {
        let Ok(dir) = tempfile::Builder::new().prefix("ara-").tempdir_in(base) else {
            continue;
        };
        let longest = dir.path().join(format!("ara-c-{}-ffffffff/s", u32::MAX));
        if longest.to_str().is_some_and(|name| name.len() <= 103) {
            return dir;
        }
    }
    panic!("no private temporary directory fits the 103-byte crash socket limit");
}
