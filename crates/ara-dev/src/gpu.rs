//! FIFO tickets are protected by OS file locks, which recover when a process dies.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

static LAST_READING: std::sync::Mutex<Option<serde_json::Value>> = std::sync::Mutex::new(None);
/// Evidence from the most recently finished guarded window in this orchestrator.
pub(crate) fn last_reading() -> Option<serde_json::Value> {
    crate::gpu_guard::reading()
        .map(|report| serde_json::json!({"reading":report,"status":report.status()}))
        .or_else(|| LAST_READING.lock().expect("guard evidence").clone())
}

fn open(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
}

struct Ticket {
    file: File,
    path: PathBuf,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
    }
}

/// A tiny queue transaction serializes allocation, stale cleanup and lock admission.
fn transaction(queue: &File, deadline: Instant) -> Result<()> {
    loop {
        match queue.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(std::fs::TryLockError::WouldBlock) => bail!("GPU queue timeout"),
            Err(error) => return Err(error.into()),
        }
    }
}

fn allocate(directory: &Path, queue: &mut File, deadline: Instant) -> Result<Ticket> {
    transaction(queue, deadline)?;
    let result = (|| {
        let mut value = String::new();
        queue.read_to_string(&mut value)?;
        // A crash during the counter write must not place a new ticket ahead of live waiters.
        let last_ticket = fs::read_dir(directory)?
            .map(|entry| {
                let path = entry?.path();
                Ok(if path.extension().is_some_and(|e| e == "ticket") {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0)
                } else {
                    0
                })
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        let sequence = value
            .trim()
            .parse::<u64>()
            .unwrap_or(0)
            .max(last_ticket)
            .checked_add(1)
            .context("GPU ticket counter exhausted")?;
        queue.seek(SeekFrom::Start(0))?;
        queue.set_len(0)?;
        write!(queue, "{sequence}")?;
        queue.sync_data()?;
        let path = directory.join(format!("{sequence:020}.ticket"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.lock()?;
        Ok(Ticket { file, path })
    })();
    queue.unlock()?;
    result
}

fn position(directory: &Path, ticket: &Ticket) -> Result<usize> {
    let mut active = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "ticket") {
            continue;
        }
        if path == ticket.path {
            active.push(path);
            continue;
        }
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        match file.try_lock() {
            Ok(()) => {
                drop(file);
                let _ = fs::remove_file(path);
            }
            Err(std::fs::TryLockError::WouldBlock) => active.push(path),
            Err(error) => return Err(error.into()),
        }
    }
    active.sort();
    active
        .iter()
        .position(|p| p == &ticket.path)
        .map(|p| p + 1)
        .context("own GPU ticket disappeared")
}

pub(crate) fn run(
    timeout: u64,
    wait_for_quiet: bool,
    policy: crate::gpu_guard::Policy,
    command: &[OsString],
) -> Result<bool> {
    // Validate output ownership before even creating baseline diagnostics.
    let root = crate::git::text(Path::new("."), ["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    crate::build_paths::Directories::from_env(&root)?;
    locked_with(timeout, wait_for_quiet, policy, || {
        eprintln!("GPU lock acquired; running {command:?}");
        crate::platform::gpu_command(command)
    })
}

/// Keep admission and crash recovery identical for direct commands and perf sessions.
pub(crate) fn locked<T>(timeout: u64, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    locked_with(
        timeout,
        true,
        crate::gpu_guard::Policy::default(),
        operation,
    )
}

/// Serializes GPU work like [`locked`] without judging foreign load. For preparation
/// that is not a measurement (freezing perf scenes): an export's import workers can
/// outlive the process that spawned them, so the guard cannot attribute them to us.
pub(crate) fn locked_unmeasured<T>(
    timeout: u64,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    locked_inner(timeout, None, operation)
}

/// Concurrent visual work shares the machine lock. FIFO timing tickets block new
/// visuals, and exclusive timing admission waits for every existing visual lease.
pub(crate) struct VisualLease {
    _file: File,
}
impl Drop for VisualLease {
    fn drop(&mut self) {
        // A forked child can hold the description until exec; closing our copy
        // must not extend visual admission beyond its owner's lifetime.
        let _ = self._file.unlock();
    }
}
pub(crate) fn visual(timeout: u64) -> Result<VisualLease> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    visual_at(&local, timeout)
}
fn visual_at(local: &Path, timeout: u64) -> Result<VisualLease> {
    let directory = local.join("ara/gpu-queue");
    fs::create_dir_all(&directory)?;
    let queue = open(&directory.join("queue.lock"))?;
    let gpu = open(&local.join("ara/gpu.lock"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        transaction(&queue, deadline)?;
        let admitted = (|| -> Result<bool> {
            let mut waiting = false;
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path.extension().is_none_or(|e| e != "ticket") {
                    continue;
                }
                let file = open(&path)?;
                match file.try_lock() {
                    Ok(()) => {
                        drop(file);
                        let _ = fs::remove_file(path);
                    }
                    Err(std::fs::TryLockError::WouldBlock) => waiting = true,
                    Err(error) => return Err(error.into()),
                }
            }
            if waiting {
                return Ok(false);
            }
            match gpu.try_lock_shared() {
                Ok(()) => Ok(true),
                Err(std::fs::TryLockError::WouldBlock) => Ok(false),
                Err(error) => Err(error.into()),
            }
        })();
        queue.unlock()?;
        if admitted? {
            return Ok(VisualLease { _file: gpu });
        }
        if Instant::now() >= deadline {
            bail!("visual GPU admission timeout after {timeout}s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(crate) fn locked_with<T>(
    timeout: u64,
    wait_for_quiet: bool,
    policy: crate::gpu_guard::Policy,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    locked_inner(timeout, Some((wait_for_quiet, policy)), operation)
}

/// Admission through the GPU queue, plus the foreign-load guard when `guard` is set.
fn locked_inner<T>(
    timeout: u64,
    guard: Option<(bool, crate::gpu_guard::Policy)>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let directory = local.join("ara/gpu-queue");
    fs::create_dir_all(&directory)?;
    let mut queue = open(&directory.join("queue.lock"))?;
    let gpu = open(&local.join("ara/gpu.lock"))?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(timeout))
        .context("--timeout too large")?;
    let ticket = allocate(&directory, &mut queue, deadline)?;
    let mut previous = 0;
    loop {
        transaction(&queue, deadline)?;
        let admitted: Result<bool> = (|| {
            let position = position(&directory, &ticket)?;
            if position != previous {
                eprintln!("GPU queue position: {position}");
                previous = position;
            }
            if position == 1 {
                match gpu.try_lock() {
                    Ok(()) => return Ok(true),
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(false)
        })();
        queue.unlock()?;
        if admitted? {
            break;
        }
        if Instant::now() >= deadline {
            bail!("GPU lock timeout after {timeout}s");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let root = crate::git::text(Path::new("."), ["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let path = crate::gpu_guard::default_path(&root);
    let guard = match guard {
        Some((wait_for_quiet, policy)) if !crate::gpu_guard::active() => {
            *LAST_READING.lock().expect("guard evidence") = None;
            match crate::gpu_guard::Guard::start(wait_for_quiet, path.clone(), policy) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    let evidence = fs::read(&path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
                    *LAST_READING.lock().expect("guard evidence") = evidence.map(|mut value| {
                        value["report"] = serde_json::json!(path);
                        value
                    });
                    return Err(error);
                }
            }
        }
        _ => None,
    };
    let mut result = operation();
    if let Some(guard) = guard {
        let report = guard.finish()?;
        *LAST_READING.lock().expect("guard evidence") =
            Some(serde_json::json!({"report":path,"reading":report,"status":report.status()}));
        eprintln!(
            "ARA_DEV_GPU_READING {}",
            serde_json::json!({"status":report.status(),"report":path,"diagnostic":report.diagnostic(),"reading":report})
        );
        if report.invalid() {
            result = Err(crate::gpu_guard::invalid_error(report.detail()));
        }
    }
    drop(gpu);
    drop(ticket);
    result
}

#[cfg(test)]
mod visual_tests {
    use super::*;
    #[test]
    fn visuals_share_but_an_existing_timing_ticket_blocks_new_visuals() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/visual-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let first = visual_at(&root, 1).unwrap();
        let second = visual_at(&root, 1).unwrap();
        // Model a descriptor inherited by an unrelated fork before exec.
        #[cfg(unix)]
        let inherited = first._file.try_clone().unwrap();
        let gpu = open(&root.join("ara/gpu.lock")).unwrap();
        assert!(matches!(
            gpu.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let directory = root.join("ara/gpu-queue");
        let mut queue = open(&directory.join("queue.lock")).unwrap();
        let ticket = allocate(
            &directory,
            &mut queue,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert!(visual_at(&root, 0).is_err());
        drop(ticket);
        drop(first);
        assert!(matches!(
            gpu.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(second);
        assert!(gpu.try_lock().is_ok());
        #[cfg(unix)]
        drop(inherited);
    }
}
