//! Cross-process, per-volume leases bound cache publication bursts. Coordinator
//! files are disposable feedback in the OS temp directory, never cache content.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
mod policy;
mod volume;
use policy::{MAX_WRITERS, Policy, now_ms};

#[derive(Debug, Clone)]
pub(super) struct Volume {
    directory: PathBuf,
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn load(file: &mut File) -> io::Result<Policy> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(133).read_to_end(&mut bytes)?;
    Ok(Policy::decode(&bytes, now_ms()))
}

fn save(file: &mut File, policy: Policy) -> io::Result<()> {
    let bytes = policy.encode();
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bytes)?;
    file.set_len(bytes.len() as u64)
}

impl Volume {
    pub fn new(cache_root: &Path) -> io::Result<Self> {
        let key = blake3::hash(volume::identity(cache_root)?.as_bytes());
        Ok(Self {
            directory: std::env::temp_dir()
                .join("ara-cache-admission-v1")
                .join(key.to_hex().as_str()),
        })
    }

    fn try_acquire(&self) -> io::Result<Option<Permit>> {
        fs::create_dir_all(&self.directory)?;
        // Each transaction opens its own handle: cloned File handles share lock
        // ownership on some platforms and cannot isolate concurrent threads.
        let mut state = open(&self.directory.join("policy"))?;
        state.lock()?;
        let mut policy = load(&mut state)?;
        let mut active = 0;
        let mut free = None;
        for i in 0..MAX_WRITERS {
            let slot = open(&self.directory.join(format!("slot-{i}")))?;
            match slot.try_lock() {
                Ok(()) => {
                    if free.is_none() {
                        free = Some(slot);
                    }
                }
                Err(fs::TryLockError::WouldBlock) => active += 1,
                Err(fs::TryLockError::Error(e)) => return Err(e),
            }
        }
        // Reset stale feedback only after every old lease has drained. A slow
        // live write must not expire its own concurrency cap.
        if active == 0 && policy.reset_if_idle(now_ms()) {
            save(&mut state, policy)?;
        }
        // Count all active slots when shrinking; removed slots must drain before
        // another writer is admitted, rather than silently exceeding the new cap.
        Ok(if active < policy.limit {
            free.map(|slot| Permit {
                slot,
                directory: self.directory.clone(),
                started: Instant::now(),
            })
        } else {
            None
        })
    }

    pub fn acquire(&self) -> io::Result<Permit> {
        let mut permit = {
            let _trace = crate::trace::Scope::new(
                "cache.write.admission",
                &self.directory.display().to_string(),
            );
            loop {
                if let Some(permit) = self.try_acquire()? {
                    break permit;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        // Queue tracing is also outside the measured publication service time.
        permit.started = Instant::now();
        Ok(permit)
    }
}

pub(super) struct Permit {
    slot: File,
    directory: PathBuf,
    started: Instant,
}

impl Permit {
    pub fn finish(self, bytes: usize) -> io::Result<()> {
        let elapsed = self.started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let mut state = open(&self.directory.join("policy"))?;
        state.lock()?;
        let mut policy = load(&mut state)?;
        let changed = policy.observe(bytes, elapsed, now_ms());
        save(&mut state, policy)?;
        // Release coordination before trace I/O: a slow diagnostic sink must not
        // serialize admission or retain a payload slot.
        drop(state);
        drop(self.slot);
        if let Some((before, p90)) = changed {
            crate::trace::event(
                "cache.write.limit",
                &self.directory.display().to_string(),
                serde_json::json!({"before":before,"after":policy.limit,"p90Ms":p90}),
            );
        }
        // Drop releases both locks even on an I/O error or unwinding. OS process
        // exit also releases slot leases; there are no persistent PID counters.
        Ok(())
    }
}

#[cfg(test)]
mod tests;
