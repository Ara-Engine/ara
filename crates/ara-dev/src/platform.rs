//! Native disk reporting has no extra executable or SDK prerequisite.

use anyhow::Result;

/// Kill the Windows process tree if its lock-owning wrapper exits unexpectedly.
#[cfg(windows)]
pub(crate) fn gpu_command(command: &[std::ffi::OsString]) -> Result<bool> {
    use anyhow::{Context, ensure};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    struct Job(HANDLE);
    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: this is the single owned handle returned by CreateJobObjectW.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    // SAFETY: null security/name arguments request an unnamed, non-inheritable job.
    let job = Job(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
    ensure!(
        !job.0.is_null(),
        "create GPU job: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: zero is a valid initialization for this Windows POD structure.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: handle is live, info and its length match the selected information class.
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
        "configure GPU job: {}",
        std::io::Error::last_os_error()
    );
    let mut child = std::process::Command::new(&command[0])
        .args(&command[1..])
        .spawn()
        .context("run GPU command")?;
    // SAFETY: job and child process handles remain live until wait completes.
    if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0 {
        let error = std::io::Error::last_os_error();
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("assign GPU command to job: {error}");
    }
    Ok(child.wait()?.success())
}

#[cfg(not(windows))]
pub(crate) fn gpu_command(command: &[std::ffi::OsString]) -> Result<bool> {
    use anyhow::Context;
    Ok(std::process::Command::new(&command[0])
        .args(&command[1..])
        .status()
        .context("run GPU command")?
        .success())
}

#[cfg(windows)]
pub(crate) fn disk(warn_gib: u64) -> Result<bool> {
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetLogicalDriveStringsW};
    let mut buffer = vec![0u16; 512];
    // SAFETY: buffer points to 512 writable UTF-16 units, with its length supplied.
    let count = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
    anyhow::ensure!(
        count > 0 && (count as usize) < buffer.len(),
        "enumerate drives: {}",
        std::io::Error::last_os_error()
    );
    let mut healthy = true;
    for drive in buffer[..count as usize]
        .split(|u| *u == 0)
        .filter(|d| !d.is_empty())
    {
        let mut root = drive.to_vec();
        root.push(0);
        let (mut free, mut total) = (0, 0);
        // SAFETY: root is NUL terminated; output pointers are valid for the call.
        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut total, ptr::null_mut()) }
            == 0
        {
            eprintln!(
                "{} unavailable: {}",
                String::from_utf16_lossy(drive),
                std::io::Error::last_os_error()
            );
            continue;
        }
        let low = free / 1_073_741_824 < warn_gib;
        healthy &= !low;
        println!(
            "{}  {:>8.2} / {:>8.2} GiB free{}",
            String::from_utf16_lossy(drive),
            free as f64 / 1_073_741_824.0,
            total as f64 / 1_073_741_824.0,
            if low { "  WARNING" } else { "" }
        );
    }
    Ok(healthy)
}

#[cfg(not(windows))]
pub(crate) fn disk(warn_gib: u64) -> Result<bool> {
    use anyhow::{Context, ensure};
    let output = std::process::Command::new("df")
        .args(["-Pk"])
        .output()
        .context("df -Pk")?;
    ensure!(output.status.success(), "df failed");
    let text = String::from_utf8(output.stdout)?;
    let mut healthy = true;
    for line in text.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let free: u64 = fields[3].parse()?;
        let low = free / 1_048_576 < warn_gib;
        healthy &= !low;
        println!(
            "{}  {:.2} GiB free{}",
            fields[5],
            free as f64 / 1_048_576.0,
            if low { "  WARNING" } else { "" }
        );
    }
    Ok(healthy)
}
