//! FIFO tickets are protected by OS file locks, which recover when a process dies.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

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

pub(crate) fn run(timeout: u64, command: &[OsString]) -> Result<bool> {
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
    eprintln!("GPU lock acquired; running {:?}", command);
    let passed = crate::platform::gpu_command(command)?;
    drop(gpu);
    drop(ticket);
    Ok(passed)
}
