//! Shared intermediates reuse Cargo fingerprints without sharing final executable names.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

pub(crate) struct Lease {
    _file: File,
}

/// Hold this through execution too: another branch must not replace live test dependencies.
pub(crate) fn acquire(deadline: Option<Instant>) -> Result<Option<Lease>> {
    let Some(path) = std::env::var_os("ARA_SHARED_BUILD_DIR").filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let directory = PathBuf::from(path);
    Ok(Some(acquire_at(&directory, deadline)?))
}

fn acquire_at(directory: &Path, deadline: Option<Instant>) -> Result<Lease> {
    fs::create_dir_all(directory)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".ara-build-admission.lock"))?;
    let start = Instant::now();
    let mut announced = false;
    loop {
        match file.try_lock() {
            Ok(()) => {
                eprintln!(
                    "Shared build admission: {:.2}s wait; {}",
                    start.elapsed().as_secs_f64(),
                    directory.display()
                );
                return Ok(Lease { _file: file });
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if deadline.is_some_and(|limit| Instant::now() >= limit) {
                    bail!("shared build admission timed out at gate deadline");
                }
                if !announced {
                    eprintln!(
                        "Waiting for shared compilation/test lane: {}",
                        directory.display()
                    );
                    announced = true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Cargo fingerprints are the shared cache. Avoid the independent sccache jobserver
/// in this lane; rustc inherits Cargo's two-job budget and registry artifacts stay fresh.
pub(crate) fn configure(command: &mut Command) {
    command.env("CARGO_BUILD_JOBS", "2");
    if std::env::var_os("ARA_SHARED_BUILD_DIR").is_some() {
        command
            .env("CARGO_INCREMENTAL", "0")
            .env("RUSTC_WRAPPER", "");
    }
}

pub(crate) fn cargo(root: &Path, args: &[OsString]) -> Result<bool> {
    if let Some(result) = crate::editor::cargo(root, args)? {
        return Ok(result);
    }
    let directories = crate::build_paths::Directories::from_env(root)?;
    let _lease = acquire(Some(Instant::now() + Duration::from_secs(2400)))?;
    let mut command = Command::new("cargo");
    command.current_dir(root).args(args);
    directories.apply(&mut command);
    configure(&mut command);
    eprintln!("Focused cargo {args:?}");
    Ok(crate::process::run(&mut command, None)
        .context("focused cargo")?
        .success())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_artifacts_remain_exclusive_until_execution_releases_the_lease() {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let directory = target.join(format!("ara-dev/admission-{}", std::process::id()));
        let first = acquire_at(&directory, None).unwrap();
        assert!(
            acquire_at(
                &directory,
                Some(Instant::now() + Duration::from_millis(100))
            )
            .is_err()
        );
        drop(first);
        assert!(acquire_at(&directory, Some(Instant::now() + Duration::from_secs(1))).is_ok());
    }
}
