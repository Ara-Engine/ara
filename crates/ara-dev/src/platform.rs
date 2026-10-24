//! Native disk reporting has no extra executable or SDK prerequisite.

use anyhow::Result;

/// Admission control leaves half the currently available physical RAM for other agents.
pub(crate) fn available_memory_mib() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: this Windows POD is zeroed and its size initialized before the call.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of_val(&status) as u32;
        // SAFETY: status is a writable structure with the expected API size.
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            return Some(status.ullAvailPhys / 1_048_576);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            return text
                .lines()
                .find_map(|line| {
                    line.strip_prefix("MemAvailable:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .map(|kib| kib / 1024);
        }
    }
    None
}

/// Windows allows renaming a loaded image, but Cargo cannot replace its old path.
#[cfg(windows)]
pub(crate) fn release_executable_path(target: &std::path::Path) -> Result<()> {
    use anyhow::Context;
    let executable = std::env::current_exe()?;
    // Installed copies are not Cargo outputs and can keep their original path.
    let Ok(target) = target.canonicalize() else {
        return Ok(());
    };
    if !executable.canonicalize()?.starts_with(target) {
        return Ok(());
    }
    let directory = executable
        .parent()
        .context("running executable directory")?
        .join("ara-dev-runners");
    if executable
        .parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|n| n == "ara-dev-runners")
    {
        return Ok(());
    }
    std::fs::create_dir_all(&directory)?;
    // Old, unlocked runner images can be removed; active ones remain protected by Windows.
    for entry in std::fs::read_dir(&directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "exe") {
            let _ = std::fs::remove_file(path);
        }
    }
    let destination = directory.join(format!("ara-dev-{}.exe", std::process::id()));
    std::fs::rename(&executable, destination)
        .context("move running gate aside so Cargo can rebuild ara-dev")?;
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn release_executable_path(_target: &std::path::Path) -> Result<()> {
    Ok(())
}

/// GPU commands use the same process-tree ownership as gate subprocesses.
pub(crate) fn gpu_command(command: &[std::ffi::OsString]) -> Result<bool> {
    let mut process = std::process::Command::new(&command[0]);
    process.args(&command[1..]);
    let root = crate::git::text(std::path::Path::new("."), ["rev-parse", "--show-toplevel"])
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    crate::build_paths::Directories::from_env(&root)?.apply(&mut process);
    Ok(crate::process::run(&mut process, None)?.success())
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
