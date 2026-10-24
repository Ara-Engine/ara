//! Programs running from an installation directory, and the Hub service's own
//! graceful shutdown (`ara hub shutdown`). Setup never terminates a process.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            CREATE_NO_WINDOW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};

/// A process whose executable lives inside the destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    /// Process ID.
    pub pid: u32,
    /// Executable file name (`ara-hub.exe`).
    pub exe: String,
    /// Full executable path.
    pub path: PathBuf,
    /// The user's Hub service (the PID in its discovery file), which
    /// [`stop_hub_service`] can stop gracefully.
    pub service: bool,
}

impl Blocker {
    /// What people call it.
    pub fn label(&self) -> String {
        match self.exe.to_ascii_lowercase().as_str() {
            "ara.exe" if self.service => "Ara Hub service".to_owned(),
            "ara.exe" => "Ara command line or MCP server".to_owned(),
            "ara-hub.exe" => "Ara Hub".to_owned(),
            "ara-hub-updater.exe" => "Ara Hub updater".to_owned(),
            "ara-editor.exe" => "Ara Editor".to_owned(),
            "ara-player.exe" => "Ara Player".to_owned(),
            _ => self.exe.clone(),
        }
    }
}

/// `Ara Hub (ara-hub.exe, PID 12), …` for messages.
pub fn describe(blockers: &[Blocker]) -> String {
    blockers
        .iter()
        .map(|blocker| format!("{} ({}, PID {})", blocker.label(), blocker.exe, blocker.pid))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every other process whose image is inside `directory`, by name then PID.
/// Missing directories and inaccessible processes yield nothing.
pub fn running_from(directory: &Path) -> Vec<Blocker> {
    let Ok(root) = std::fs::canonicalize(directory) else {
        return Vec::new();
    };
    let root = win32(&root).to_lowercase();
    let root = format!("{}\\", root.trim_end_matches('\\'));
    let service = hub_service_pid();
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked, iterated with a correctly sized
    // entry and closed exactly once.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let pid = entry.th32ProcessID;
            if pid != 0
                && pid != std::process::id()
                && let Some(path) = image(pid)
                && path.to_lowercase().starts_with(&root)
            {
                let path = PathBuf::from(path);
                found.push(Blocker {
                    pid,
                    exe: path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path,
                    service: service == Some(pid),
                });
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    found.sort_by_key(|blocker| (blocker.label(), blocker.pid));
    found
}

/// A canonical path without the `\\?\` prefix, as process image paths are reported.
fn win32(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
}

fn image(pid: u32) -> Option<String> {
    // SAFETY: limited query access only; the handle is closed before returning and
    // the buffer length is passed in and updated by the call.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32_768];
        let mut len = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
                != 0;
        CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// The running Hub service's PID from `<config>/hub/.ara/daemon.json`, where the
/// config is `ARA_CONFIG_DIR` or `%APPDATA%\Ara` (as `ara_hub_service::config_dir`).
fn hub_service_pid() -> Option<u32> {
    let config = std::env::var_os("ARA_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("Ara"))
        })?;
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(config.join("hub/.ara/daemon.json")).ok()?).ok()?;
    discovery["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
}

/// Asks the Hub service to stop through its own `ara hub shutdown` (project
/// processes stay open) and waits up to 15 s for it to exit.
pub fn stop_hub_service(service: &Blocker) -> Result<()> {
    let output = Command::new(&service.path)
        .args(["hub", "shutdown"])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("ARA-EXP-INSTALL-007: could not run `ara hub shutdown`")?;
    ensure!(
        output.status.success(),
        "ARA-EXP-INSTALL-007: `ara hub shutdown` failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    // SAFETY: synchronize access only; the handle is closed after a bounded wait.
    let stopped = unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, service.pid);
        if process.is_null() {
            true
        } else {
            let status = WaitForSingleObject(process, 15_000);
            CloseHandle(process);
            status == WAIT_OBJECT_0
        }
    };
    ensure!(
        stopped,
        "ARA-EXP-INSTALL-007: the Ara Hub service (PID {}) did not stop within 15 seconds",
        service.pid
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_program_started_from_the_folder_blocks_it() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Ara 0.0.1-alpha");
        std::fs::create_dir(&folder).unwrap();
        let system = std::env::var_os("SystemRoot").unwrap();
        let ping = PathBuf::from(system).join("System32/PING.EXE");
        std::fs::copy(&ping, folder.join("ara-editor.exe")).unwrap();
        assert!(running_from(&folder).is_empty());
        let mut child = Command::new(folder.join("ara-editor.exe"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        // A folder whose name is a prefix of the real one is not affected.
        let prefix = root.path().join("Ara 0.0.1");
        std::fs::create_dir(&prefix).unwrap();
        let found = running_from(&folder);
        let beside = running_from(&prefix);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].pid, child.id());
        assert_eq!(found[0].label(), "Ara Editor");
        assert!(!found[0].service);
        assert!(beside.is_empty(), "{beside:?}");
        assert!(running_from(&root.path().join("missing")).is_empty());
    }
}
