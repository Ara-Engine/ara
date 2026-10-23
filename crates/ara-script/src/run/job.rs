//! OS containment for the `ara_run` worker process.
//!
//! Windows: a job object caps the worker's committed memory (allocations past it fail)
//! and kills it when the supervisor closes the job. Linux: the supervisor polls the
//! worker's resident memory. Everywhere: `DOTNET_GCHeapHardLimit` makes the managed heap
//! throw `OutOfMemoryException` at the limit, and the supervisor kills on violation.

use std::process::Child;

/// Memory containment for one worker process.
pub(crate) struct Job {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: the job handle is a kernel handle usable from any thread; only the owning
// supervisor uses it.
unsafe impl Send for Job {}

impl Job {
    /// Puts `child` in a job capped at `memory` committed bytes (Windows); elsewhere a
    /// no-op. `None` if the job could not be set up (the worker then runs uncontained).
    pub fn new(child: &Child, memory: u64) -> Option<Job> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectExtendedLimitInformation, SetInformationJobObject,
            };
            // SAFETY: plain Win32 calls; `info` is a valid, fully initialized struct of the
            // size passed, and `child`'s handle is live for the call.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                // Owns the handle from here: an early return closes it.
                let job = Job { handle };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags =
                    JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                info.ProcessMemoryLimit = usize::try_from(memory).unwrap_or(usize::MAX);
                let limited = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
                );
                if limited == 0 || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0 {
                    return None;
                }
                Some(job)
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (child, memory);
            Some(Job {})
        }
    }

    /// The memory the worker has committed now (Windows), or its resident set (Linux).
    /// Unlike the job's peak this falls when the worker frees memory, so one run's
    /// spike does not count against the next run on the same worker.
    pub fn committed(&self, child: &Child) -> Option<u64> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::ProcessStatus::{
                GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            };
            // SAFETY: `counters` is a writable struct of the size passed (the EX layout
            // starts with the base one); `child`'s handle is live.
            unsafe {
                let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
                let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0);
                counters.cb = size;
                let ok = GetProcessMemoryInfo(
                    child.as_raw_handle(),
                    std::ptr::from_mut(&mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
                    size,
                );
                (ok != 0).then_some(counters.PrivateUsage as u64)
            }
        }
        #[cfg(not(windows))]
        {
            self.peak(child)
        }
    }

    /// The most memory the worker has ever committed (Windows), or its current resident
    /// set (Linux). Used to tell a worker that died at the job's cap from a crash.
    pub fn peak(&self, child: &Child) -> Option<u64> {
        #[cfg(windows)]
        {
            let _ = child;
            use windows_sys::Win32::System::JobObjects::{
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                QueryInformationJobObject,
            };
            // SAFETY: `info` is a writable struct of the size passed; the handle is live.
            unsafe {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let ok = QueryInformationJobObject(
                    self.handle,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_mut(&mut info).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
                    std::ptr::null_mut(),
                );
                (ok != 0).then_some(info.PeakProcessMemoryUsed as u64)
            }
        }
        #[cfg(target_os = "linux")]
        {
            let status = std::fs::read_to_string(format!("/proc/{}/status", child.id())).ok()?;
            let kb: u64 = status
                .lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))?
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()?;
            Some(kb * 1024)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = child;
            None
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: the handle came from CreateJobObjectW and is closed once; closing it
        // kills the worker (KILL_ON_JOB_CLOSE).
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}
