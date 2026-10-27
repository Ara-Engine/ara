//! A shared child has one process owner. Frame-time checks only read atomics;
//! kill/reap happens on the dedicated owner thread, never behind a frame lock.
use std::{
    io,
    process::{Child, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

enum Kind {
    Separate(Child),
    Shared {
        state: Arc<State>,
        stop: mpsc::Sender<()>,
    },
}
struct State {
    pid: u32,
    raw: AtomicU32,
    exited: AtomicBool,
    #[cfg(windows)]
    handle: std::os::windows::io::OwnedHandle,
}
/// A process handle for one logical player. Shared players refer to the same
/// owned child; stopping a failed member stops that shared process and all members.
pub struct ChildHandle(Kind);
impl From<Child> for ChildHandle {
    fn from(child: Child) -> Self {
        Self(Kind::Separate(child))
    }
}
impl ChildHandle {
    pub(super) fn share(mut child: Child, count: usize) -> io::Result<Vec<Self>> {
        #[cfg(windows)]
        let handle = {
            use std::os::windows::io::AsHandle;
            match child.as_handle().try_clone_to_owned() {
                Ok(handle) => handle,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
        };
        let state = Arc::new(State {
            pid: child.id(),
            raw: AtomicU32::new(0),
            exited: AtomicBool::new(false),
            #[cfg(windows)]
            handle,
        });
        let (transfer, receive) = mpsc::channel::<Child>();
        let (stop, requests) = mpsc::channel();
        let observed = Arc::clone(&state);
        if let Err(error) = thread::Builder::new()
            .name("ara-play-shared-process".into())
            .spawn(move || {
                let Ok(mut child) = receive.recv() else {
                    return;
                };
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break Some(status),
                        Err(_) => break None,
                        Ok(None) => {}
                    }
                    match requests.recv_timeout(Duration::from_millis(10)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = child.kill();
                            break child.wait().ok();
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                };
                // A wait error must still release every waiter and retire the child.
                let status = status.unwrap_or_else(|| {
                    let _ = child.kill();
                    child.wait().unwrap_or_else(|_| failure_status())
                });
                observed.raw.store(raw_status(status), Ordering::Relaxed);
                observed.exited.store(true, Ordering::Release);
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = transfer.send(child) {
            child = error.0;
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(
                "shared process owner stopped before transfer",
            ));
        }
        Ok((0..count)
            .map(|_| {
                Self(Kind::Shared {
                    state: Arc::clone(&state),
                    stop: stop.clone(),
                })
            })
            .collect())
    }
    /// The actual child PID; identical for every member of a shared group.
    pub fn id(&self) -> u32 {
        match &self.0 {
            Kind::Separate(child) => child.id(),
            Kind::Shared { state, .. } => state.pid,
        }
    }
    /// Whether another logical player shares this process's lifetime.
    pub fn is_shared(&self) -> bool {
        matches!(self.0, Kind::Shared { .. })
    }
    /// Poll the process without waiting. Shared groups use a cached exit status.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match &mut self.0 {
            Kind::Separate(child) => child.try_wait(),
            Kind::Shared { state, .. } => Ok(state
                .exited
                .load(Ordering::Acquire)
                .then(|| status_from_raw(state.raw.load(Ordering::Relaxed)))),
        }
    }
    /// Request termination of this exact owned process (and every shared member).
    pub fn kill(&mut self) -> io::Result<()> {
        match &mut self.0 {
            Kind::Separate(child) => child.kill(),
            Kind::Shared { stop, .. } => {
                let _ = stop.send(());
                Ok(())
            }
        }
    }
    /// Wait for the process owner to reap the child. Only shutdown/fault workers call this.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        match &mut self.0 {
            Kind::Separate(child) => child.wait(),
            Kind::Shared { state, .. } => {
                while !state.exited.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(status_from_raw(state.raw.load(Ordering::Relaxed)))
            }
        }
    }
}
impl ara_gpu::shared::SurfaceProducer for ChildHandle {
    fn process_id(&self) -> u32 {
        self.id()
    }
    #[cfg(windows)]
    fn process_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        use std::os::windows::io::AsHandle;
        match &self.0 {
            Kind::Separate(child) => child.as_handle(),
            Kind::Shared { state, .. } => state.handle.as_handle(),
        }
    }
}
#[cfg(windows)]
fn raw_status(status: ExitStatus) -> u32 {
    use std::os::windows::process::ExitStatusExt;
    status.into_raw()
}
#[cfg(unix)]
fn raw_status(status: ExitStatus) -> u32 {
    use std::os::unix::process::ExitStatusExt;
    status.into_raw() as u32
}
#[cfg(windows)]
fn status_from_raw(raw: u32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(raw)
}
#[cfg(unix)]
fn status_from_raw(raw: u32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(raw as i32)
}
fn failure_status() -> ExitStatus {
    status_from_raw(if cfg!(unix) { 1 << 8 } else { 1 })
}
