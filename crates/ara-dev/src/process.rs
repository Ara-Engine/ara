//! A gate deadline includes Cargo lock waits and owns the spawned process tree.

use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use anyhow::Result;

pub(crate) enum Outcome {
    Exited(ExitStatus),
    TimedOut,
}

impl Outcome {
    pub(crate) fn success(&self) -> bool {
        matches!(self, Self::Exited(status) if status.success())
    }
}

struct Tree {
    child: Child,
    #[cfg(windows)]
    job: Option<windows::Job>,
}

impl Drop for Tree {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        if self.child.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            // SAFETY: this child created its own process group with this live PID.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(crate) fn run(command: &mut Command, deadline: Option<Instant>) -> Result<Outcome> {
    run_observed(command, deadline, || Ok(()))
}

/// Observe bounded status files while retaining the same child-tree cleanup guarantees.
pub(crate) fn run_observed(
    command: &mut Command,
    deadline: Option<Instant>,
    mut observe: impl FnMut() -> Result<()>,
) -> Result<Outcome> {
    if deadline.is_some_and(|limit| Instant::now() >= limit) {
        return Ok(Outcome::TimedOut);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    let job = windows::Job::new()?;
    let child = command.spawn()?;
    let mut tree = Tree {
        child,
        #[cfg(windows)]
        job: Some(job),
    };
    #[cfg(windows)]
    tree.job
        .as_ref()
        .expect("job exists until tree cleanup")
        .assign(&tree.child)?;
    loop {
        observe()?;
        if let Some(status) = tree.child.try_wait()? {
            return Ok(Outcome::Exited(status));
        }
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Ok(Outcome::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
mod windows {
    use anyhow::{Result, ensure};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    pub(super) struct Job(HANDLE);

    impl Job {
        pub(super) fn new() -> Result<Self> {
            // SAFETY: null arguments request an unnamed, non-inheritable job.
            let job = Self(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
            ensure!(
                !job.0.is_null(),
                "create command job: {}",
                std::io::Error::last_os_error()
            );
            // SAFETY: zero initializes this Windows POD structure.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the live handle, structure type and length match this API.
            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                )
            };
            ensure!(
                configured != 0,
                "configure command job: {}",
                std::io::Error::last_os_error()
            );
            Ok(job)
        }

        pub(super) fn assign(&self, child: &Child) -> Result<()> {
            // SAFETY: both process and job handles are live until command cleanup.
            let assigned = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) };
            ensure!(
                assigned != 0,
                "assign command job: {}",
                std::io::Error::last_os_error()
            );
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: this owns the handle returned by CreateJobObjectW.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[test]
    fn deadline_terminates_a_running_command() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "process::tests::sleep_helper"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let start = Instant::now();
        let outcome = run(&mut command, Some(start + Duration::from_millis(500))).unwrap();
        assert!(matches!(outcome, Outcome::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    #[ignore = "subprocess helper for the deadline test"]
    fn sleep_helper() {
        std::thread::sleep(Duration::from_secs(30));
    }
}
