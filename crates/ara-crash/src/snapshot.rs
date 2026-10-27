//! Panic snapshots: a minidump paired with a panic record while the process may keep
//! running. minidumper's dump request ends its client, so Windows and Linux send the
//! crash context as an ordinary message, and macOS (whose context is Mach ports that
//! only a Mach message can carry) dumps through a second, short-lived client.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(crate) fn thread(context: &crash_handler::CrashContext) -> u32 {
    #[cfg(windows)]
    return context.thread_id;
    #[cfg(target_os = "linux")]
    return context.tid as u32;
    #[cfg(target_os = "macos")]
    return context.thread;
}

pub(crate) fn current_thread() -> u32 {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        // SAFETY: this parameterless OS query has no caller invariants.
        unsafe { GetCurrentThreadId() }
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: gettid takes no arguments and returns this calling thread's ID.
        unsafe { libc::syscall(libc::SYS_gettid) as u32 }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: pthread_self is this live thread's handle; the lookup adds no port
        // reference. The name matches the thread port the exception message carries.
        unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) }
    }
}

#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn request(client: &minidumper::Client, context: &crash_handler::CrashContext) -> bool {
    #[cfg(windows)]
    let bytes = {
        let mut bytes = [0u8; 20];
        bytes[..8].copy_from_slice(&(context.exception_pointers as u64).to_le_bytes());
        bytes[8..12].copy_from_slice(&context.process_id.to_le_bytes());
        bytes[12..16].copy_from_slice(&context.thread_id.to_le_bytes());
        bytes[16..20].copy_from_slice(&context.exception_code.to_le_bytes());
        bytes
    };
    #[cfg(target_os = "linux")]
    let bytes = context.as_bytes();
    // Ping is acknowledged after the monitor finishes writing, while the sender's
    // exception pointers remain live on this stack. No callback allocation.
    client.send_message(2, bytes).is_ok() && client.ping().is_ok()
}

/// The dump path paired with panic record `name` (`<stem>.crash.json` -> `<stem>.dmp`),
/// which `symbols::symbolize` finds beside the record.
pub(crate) fn path(directory: &Path, name: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !name.contains(['/', '\\', ':']) && name.ends_with(".crash.json"),
        "invalid panic snapshot name"
    );
    let stem = name
        .strip_suffix(".crash.json")
        .context("invalid panic record")?;
    anyhow::ensure!(
        !stem.is_empty()
            && stem.len() < 80
            && stem.bytes().all(|b| b.is_ascii_digit() || b == b'-'),
        "invalid panic snapshot identifier"
    );
    Ok(directory.join(format!("{stem}.dmp")))
}

#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn write(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let path = path(directory, name)?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(&path)?;
    let result = dump(bytes, &mut file);
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return result;
    }
    file.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn dump(bytes: &[u8], file: &mut std::fs::File) -> Result<()> {
    anyhow::ensure!(bytes.len() == 20, "invalid Windows snapshot context");
    let context = minidump_writer::crash_context::CrashContext {
        // Remote pointers, kept alive until the worker acknowledges the ping.
        exception_pointers: u64::from_le_bytes(bytes[..8].try_into()?) as *const _,
        process_id: u32::from_le_bytes(bytes[8..12].try_into()?),
        thread_id: u32::from_le_bytes(bytes[12..16].try_into()?),
        exception_code: i32::from_le_bytes(bytes[16..20].try_into()?),
    };
    minidump_writer::minidump_writer::MinidumpWriter::dump_crash_context(&context, None, file)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn dump(bytes: &[u8], file: &mut std::fs::File) -> Result<()> {
    let context = minidump_writer::crash_context::CrashContext::from_bytes(bytes)
        .context("invalid Linux snapshot context")?;
    let mut writer =
        minidump_writer::minidump_writer::MinidumpWriterConfig::new(context.pid, context.tid);
    writer.set_crash_context(minidump_writer::CrashContextExt { inner: context });
    writer.write(file)?;
    Ok(())
}

/// A macOS panic snapshot: the panic hook arms a connected client, raises a simulated
/// exception and waits; the crash callback dumps through that client and wakes it.
/// crash-handler delivers macOS exceptions on its own thread, never in a signal
/// handler, so the callback may lock here.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub(crate) struct Pending {
    client: std::sync::Mutex<Option<minidumper::Client>>,
    done: std::sync::Mutex<bool>,
    finished: std::sync::Condvar,
}

#[cfg(target_os = "macos")]
impl Pending {
    /// Hold the client the next simulated exception dumps through.
    pub(crate) fn arm(&self, client: minidumper::Client) {
        if let Ok(mut done) = self.done.lock() {
            *done = false;
        }
        if let Ok(mut slot) = self.client.lock() {
            *slot = Some(client);
        }
    }
    /// Crash-callback side: dump through the armed client (the monitor then drops
    /// it), and wake the waiting panic hook either way.
    pub(crate) fn request(&self, context: &crash_handler::CrashContext) -> bool {
        let client = self.client.try_lock().ok().and_then(|mut slot| slot.take());
        let dumped = client.is_some_and(|client| client.request_dump(context).is_ok());
        if let Ok(mut done) = self.done.lock() {
            *done = true;
            self.finished.notify_all();
        }
        dumped
    }
    /// Panic-hook side: wait up to `timeout` for the callback, then drop an unused
    /// client. True when the snapshot request ran.
    pub(crate) fn wait(&self, timeout: std::time::Duration) -> bool {
        let done = self
            .done
            .lock()
            .ok()
            .and_then(|done| {
                self.finished
                    .wait_timeout_while(done, timeout, |done| !*done)
                    .ok()
            })
            .is_some_and(|(done, _)| *done);
        if let Ok(mut slot) = self.client.lock() {
            slot.take();
        }
        done
    }
}
