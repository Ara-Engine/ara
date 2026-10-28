//! Cancellable process lifetime observation. Windows waits on kernel objects;
//! discovery files are never used as an exit signal.
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub(super) struct Watch {
    exited: Arc<AtomicBool>,
    platform: platform::Watch,
}
impl Watch {
    pub(super) fn new(pid: u32, exited: impl FnOnce() + Send + 'static) -> io::Result<Self> {
        let flag = Arc::new(AtomicBool::new(false));
        let state = flag.clone();
        Ok(Self {
            exited: flag,
            platform: platform::Watch::new(pid, move || {
                state.store(true, Ordering::Release);
                exited();
            })?,
        })
    }
    pub(super) fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
            || self.platform.wait(Duration::ZERO).is_ok_and(|done| done)
    }
    pub(super) fn wait(&self, timeout: Duration) -> io::Result<bool> {
        self.platform.wait(timeout)
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{
            CreateEventW, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, SetEvent,
            WaitForMultipleObjects, WaitForSingleObject,
        },
    };

    pub(super) struct Watch {
        process: Arc<OwnedHandle>,
        cancel: Arc<OwnedHandle>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Watch {
        pub(super) fn new(pid: u32, exited: impl FnOnce() + Send + 'static) -> io::Result<Self> {
            // SAFETY: Request synchronization only, without handle inheritance.
            let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: Successful OpenProcess transfers one owned handle.
            let process = Arc::new(unsafe { OwnedHandle::from_raw_handle(process) });
            // SAFETY: Null security/name, manual reset, initially unsignalled.
            let cancel = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if cancel.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: Successful CreateEventW transfers one owned handle.
            let cancel = Arc::new(unsafe { OwnedHandle::from_raw_handle(cancel) });
            let observed = process.clone();
            let stop = cancel.clone();
            let worker = thread::Builder::new()
                .name(format!("hub-process-{pid}"))
                .spawn(move || {
                    let handles = [observed.as_raw_handle(), stop.as_raw_handle()];
                    // SAFETY: Both handles are owned by this closure for the entire wait.
                    if unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) }
                        == WAIT_OBJECT_0
                    {
                        exited();
                    }
                })?;
            Ok(Self {
                process,
                cancel,
                worker: Some(worker),
            })
        }
        pub(super) fn wait(&self, timeout: Duration) -> io::Result<bool> {
            // SAFETY: Owned process handle remains live; the wait is bounded.
            match unsafe {
                WaitForSingleObject(
                    self.process.as_raw_handle(),
                    timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32,
                )
            } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
    impl Drop for Watch {
        fn drop(&mut self) {
            // SAFETY: This object owns the manual-reset event until after the join.
            unsafe {
                SetEvent(self.cancel.as_raw_handle());
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

// Other platforms retain a bounded, cancellable PID-liveness fallback. Windows
// (the release platform) never polls either PIDs or discovery files.
#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::sync::mpsc::{self, Sender};
    pub(super) struct Watch {
        pid: u32,
        cancel: Sender<()>,
        worker: Option<thread::JoinHandle<()>>,
    }
    fn alive(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(')')
                    .map(|(_, state)| state.trim_start().starts_with('Z'))
            })
            .unwrap_or(false)
        {
            return false;
        }
        // An exited process its parent has not reaped yet (a zombie) still answers
        // signal zero. On macOS, proc_pidinfo fails with ESRCH for zombies, but the
        // kernel process table (sysctl KERN_PROC_PID) reports their SZOMB state.
        #[cfg(target_os = "macos")]
        {
            // libc does not bind Apple's `struct kinfo_proc`; its 64-bit layout is
            // frozen ABI: 648 bytes, with `kp_proc.p_stat` (a char) at byte 36.
            const KINFO_PROC_SIZE: usize = 648;
            const P_STAT_OFFSET: usize = 36;
            let mut mib = [
                libc::CTL_KERN,
                libc::KERN_PROC,
                libc::KERN_PROC_PID,
                pid as libc::c_int,
            ];
            let mut info = [0u8; KINFO_PROC_SIZE];
            let mut size = KINFO_PROC_SIZE;
            // SAFETY: `mib` names a 4-level KERN_PROC_PID query and `info` is a writable
            // buffer of `size` bytes, the most sysctl copies out; no new value is set.
            let status = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    mib.len() as libc::c_uint,
                    info.as_mut_ptr().cast(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            // A pid with no process copies out nothing (size 0).
            if status == 0
                && size == KINFO_PROC_SIZE
                && u32::from(info[P_STAT_OFFSET]) == libc::SZOMB
            {
                return false;
            }
        }
        // SAFETY: Signal zero only checks a process, without sending a signal.
        unsafe {
            libc::kill(pid as libc::pid_t, 0) == 0
                || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
    impl Watch {
        pub(super) fn new(pid: u32, exited: impl FnOnce() + Send + 'static) -> io::Result<Self> {
            if !alive(pid) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "project process already exited",
                ));
            }
            let (cancel, stop) = mpsc::channel();
            let worker = thread::Builder::new()
                .name(format!("hub-process-{pid}"))
                .spawn(move || {
                    loop {
                        if !alive(pid) {
                            exited();
                            return;
                        }
                        if stop.recv_timeout(Duration::from_millis(100))
                            != Err(mpsc::RecvTimeoutError::Timeout)
                        {
                            return;
                        }
                    }
                })?;
            Ok(Self {
                pid,
                cancel,
                worker: Some(worker),
            })
        }
        pub(super) fn wait(&self, timeout: Duration) -> io::Result<bool> {
            let deadline = std::time::Instant::now() + timeout;
            while alive(self.pid) {
                if std::time::Instant::now() >= deadline {
                    return Ok(false);
                }
                thread::sleep(Duration::from_millis(10));
            }
            Ok(true)
        }
    }
    impl Drop for Watch {
        fn drop(&mut self) {
            let _ = self.cancel.send(());
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A project process the Hub did not spawn (the fixture editor is the test's
        /// child) stays a zombie after it exits until its parent reaps it; it has exited.
        #[test]
        fn an_exited_unreaped_child_is_not_alive() {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .spawn()
                .expect("child");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while alive(child.id()) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "an exited, unreaped child still reads as alive"
                );
                thread::sleep(Duration::from_millis(10));
            }
            child.wait().expect("reap");
        }
    }
}
