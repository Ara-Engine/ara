//! Registration: the installed package's `install.ps1 -RegisterOnly`, run through
//! System32's Windows PowerShell without a console window, output captured.
use anyhow::{Context, Result, ensure};
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use windows_sys::Win32::System::{
    SystemInformation::GetSystemDirectoryW, Threading::CREATE_NO_WINDOW,
};

/// Runs registration for `destination`; returns stdout and stderr. A failure keeps
/// the installed files (`install.ps1 -RegisterOnly` can be rerun) and carries the
/// output after a blank line, so the window shows it under Copy details.
pub fn run(destination: &Path, silent: bool, no_system_integration: bool) -> Result<String> {
    let mut command = Command::new(powershell()?);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(destination.join("install.ps1"))
        .arg("-RegisterOnly")
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    if silent {
        command.arg("-Silent");
    }
    if no_system_integration {
        command.arg("-NoSystemIntegration");
    }
    let output = command
        .output()
        .context("starting Windows PowerShell for registration")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_owned();
    ensure!(
        output.status.success(),
        "ARA-EXP-INSTALL-010: registration failed ({}). The files remain at {}; run install.ps1 -RegisterOnly there to retry\n\n{text}",
        output.status,
        destination.display()
    );
    Ok(text)
}

/// `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`, never a
/// `powershell.exe` found beside the downloaded setup.
fn powershell() -> Result<PathBuf> {
    let mut buffer = [0u16; 260];
    // SAFETY: the buffer is writable for its full length, which is passed.
    let len = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    ensure!(
        len > 0 && len < buffer.len(),
        "cannot locate the Windows system directory"
    );
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..len]))
        .join("WindowsPowerShell/v1.0/powershell.exe"))
}
