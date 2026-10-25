//! Standalone Hub transaction. The worker runs outside the directory being swapped.
use crate::{
    Cancel, Error, HASH, Kind, MANIFEST, ROLLBACK, extract, sha256_file, trusted_keys, verify,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Durable restart request; the updater authenticates the archive again before extraction.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    /// Standalone Hub install directory (must contain hub.json, never engine.json).
    pub current: PathBuf,
    /// Downloaded archive still covered by the signed manifest.
    pub archive: PathBuf,
    /// Exact signed JSON bytes.
    pub manifest: String,
    /// Hexadecimal detached signature.
    pub signature: String,
    /// Hub configuration to reuse after restart.
    pub config: PathBuf,
    /// Separate engine/service executable; never in current or its backup.
    pub ara: PathBuf,
}
/// Atomically swap two sibling directories. Health failure restores the old tree.
/// Callers must hold an exclusive transaction lock and stop the old Hub first.
pub fn swap(
    current: &Path,
    staged: &Path,
    health: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    if current.parent() != staged.parent() || !current.is_dir() || !staged.is_dir() {
        return Err(Error::new(
            MANIFEST,
            "update directories must be existing siblings",
        ));
    }
    let backup = current.with_extension("old");
    if backup.exists() {
        return Err(Error::new(
            ROLLBACK,
            "previous Hub backup exists; recover it before updating",
        ));
    }
    fs::rename(current, &backup)?;
    if let Err(error) = fs::rename(staged, current) {
        fs::rename(&backup, current)?;
        return Err(error.into());
    }
    if let Err(error) = health(current) {
        fs::rename(current, staged)?;
        fs::rename(&backup, current)?;
        return Err(Error::new(
            ROLLBACK,
            format!("new Hub failed its startup health check; previous version restored: {error}"),
        ));
    }
    fs::remove_dir_all(backup)?;
    Ok(())
}
/// Wait for the original GUI process to exit without stopping any service/editor.
pub fn wait_for_parent(pid: u32) -> Result<(), Error> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        // SAFETY: Request only synchronization access to the supplied process ID.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Ok(());
        }
        // SAFETY: OpenProcess returned a valid owned process handle; bounded wait.
        let status = unsafe { WaitForSingleObject(handle, 120_000) };
        // SAFETY: Release our owned handle, never terminate the target process.
        unsafe { CloseHandle(handle) };
        if status != 0 {
            return Err(Error::new(ROLLBACK, "Hub did not exit within two minutes"));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        return Err(Error::new(
            MANIFEST,
            "Hub process updater currently supports Windows only",
        ));
    }
    #[allow(unreachable_code)]
    Ok(())
}
/// Verify, extract, swap, launch and monitor the real GUI's startup acknowledgment.
pub fn apply(plan: &Plan) -> Result<(), Error> {
    apply_trusted(plan, &trusted_keys())
}
/// Exercise the complete process handshake with synthetic keys on a local fake feed.
#[cfg(any(test, feature = "test-support"))]
pub fn apply_for_test(plan: &Plan, keys: &[String]) -> Result<(), Error> {
    apply_trusted(plan, keys)
}
fn apply_trusted(plan: &Plan, keys: &[String]) -> Result<(), Error> {
    let result = apply_inner(plan, keys);
    let target = verify(plan.manifest.as_bytes(), plan.signature.as_bytes(), keys)
        .ok()
        .map(|release| release.manifest().version.clone());
    let current = fs::read(plan.current.join("hub.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["version"].as_str().map(str::to_owned));
    if result.is_err()
        && (target.is_none() || current != target)
        && plan.current.join("ara-hub.exe").is_file()
    {
        let _ = hub_command(&plan.current, plan).spawn();
    }
    result
}
fn apply_inner(plan: &Plan, keys: &[String]) -> Result<(), Error> {
    let release = verify(plan.manifest.as_bytes(), plan.signature.as_bytes(), keys)?;
    if release.manifest().kind != Kind::Hub
        || plan.current.join("engine.json").exists()
        || !plan.current.join("hub.json").is_file()
        || plan.ara.starts_with(&plan.current)
    {
        return Err(Error::new(
            MANIFEST,
            "self-update requires a standalone Hub and an external versioned service",
        ));
    }
    let asset = release.archive()?;
    if fs::metadata(&plan.archive)?.len() != asset.size
        || sha256_file(&plan.archive)? != asset.sha256
    {
        return Err(Error::new(
            HASH,
            "staged Hub archive differs from signed release",
        ));
    }
    let parent = plan
        .current
        .parent()
        .ok_or_else(|| Error::new(MANIFEST, "Hub install has no parent"))?;
    let lock = fs::File::create(parent.join(".ara-hub-update.lock"))?;
    lock.try_lock()
        .map_err(|_| Error::new(ROLLBACK, "another Hub update is already running"))?;
    let staged = parent.join(".ara-hub-new");
    if staged.exists() {
        return Err(Error::new(
            ROLLBACK,
            "unfinished Hub update exists; inspect .ara-hub-new before retrying",
        ));
    }
    extract(&plan.archive, &staged, &Cancel::default())?;
    if [
        "ara-hub.exe",
        "ara-hub-updater.exe",
        "bootstrap/ara.exe",
        "bootstrap/service.json",
        "THIRD_PARTY_NOTICES.txt",
        "hub.json",
    ]
    .iter()
    .any(|file| !staged.join(file).is_file())
        || staged.join("engine.json").exists()
    {
        fs::remove_dir_all(&staged)?;
        return Err(Error::new(MANIFEST, "archive is not a standalone Hub"));
    }
    let marker = fs::read(staged.join("hub.json"))
        .map_err(Error::from)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).map_err(Error::from));
    let marker = match marker {
        Ok(marker) => marker,
        Err(error) => {
            let _ = fs::remove_dir_all(&staged);
            return Err(error);
        }
    };
    if marker["format"] != "ara.hub/1" || marker["version"] != release.manifest().version {
        fs::remove_dir_all(&staged)?;
        return Err(Error::new(
            MANIFEST,
            "Hub archive version differs from signed manifest",
        ));
    }
    let marker = plan
        .config
        .join(format!("hub-health-{}", std::process::id()));
    if marker.exists() {
        fs::remove_file(&marker)?;
    }
    let result = swap(&plan.current, &staged, |current| {
        let mut command = hub_command(current, plan);
        command.arg("--update-health").arg(&marker);
        let mut child = command.spawn()?;
        let health = (|| {
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                if child.try_wait()?.is_some() {
                    return Err(Error::new(ROLLBACK, "new Hub exited during startup"));
                }
                if marker.is_file() {
                    let _ = fs::remove_file(&marker);
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    child.kill()?;
                    child.wait()?;
                    return Err(Error::new(ROLLBACK, "new Hub never acknowledged startup"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })();
        if health.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        health
    });
    let current_version = fs::read(plan.current.join("hub.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["version"].as_str().map(str::to_owned));
    if result.as_ref().is_err_and(|error| error.code == ROLLBACK)
        && !plan.current.with_extension("old").exists()
        && staged.exists()
        && current_version.as_deref() != Some(release.manifest().version.as_str())
    {
        let _ = fs::remove_dir_all(&staged);
    }
    result
}
fn hub_command(current: &Path, plan: &Plan) -> Command {
    let mut command = Command::new(current.join("ara-hub.exe"));
    command
        .env("ARA_CONFIG_DIR", &plan.config)
        .arg("--config-dir")
        .arg(&plan.config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updated_hub_selects_its_new_bootstrap_instead_of_the_previous_service() {
        let plan = Plan {
            current: "Hub".into(),
            archive: "download.zip".into(),
            manifest: String::new(),
            signature: String::new(),
            config: "target/update-config".into(),
            ara: "Services/old/ara.exe".into(),
        };
        let command = hub_command(&plan.current, &plan);
        assert!(
            !command
                .get_args()
                .any(|arg| arg == "--ara" || arg == plan.ara.as_os_str())
        );
    }
}
