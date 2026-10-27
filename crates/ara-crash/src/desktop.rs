//! Local crash capture for installed desktop applications. Never uploads reports.
mod seen;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Per-user dump directory, overridable for isolated launch tests.
pub fn directory() -> Result<PathBuf> {
    std::env::var_os("ARA_CRASH_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Ara/crashes"))
        })
        .or_else(|| {
            // macOS keeps per-user application data under Library, beside Ara's settings.
            cfg!(target_os = "macos")
                .then(|| std::env::var_os("HOME"))
                .flatten()
                .map(|dir| PathBuf::from(dir).join("Library/Application Support/Ara/crashes"))
        })
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME").map(|dir| PathBuf::from(dir).join("ara/crashes"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".local/state/ara/crashes"))
        })
        .context("cannot locate the user crash directory; set ARA_CRASH_DIR")
}

/// Installs capture and optionally offers to open reports left by the previous launch.
/// The caller handles `monitor_from_args` first and retains the returned guard.
pub fn start(application: &str, interactive: bool) -> Result<crate::Capture> {
    let directory = directory()?;
    std::fs::create_dir_all(&directory)?;
    seen::acknowledge(&directory, application, interactive, offer_folder)?;
    crate::Capture::start(
        &directory,
        &format!("{application}-{}", env!("CARGO_PKG_VERSION")),
    )
}

#[cfg(windows)]
fn offer_folder(directory: &std::path::Path) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            title: *const u16,
            flags: u32,
        ) -> i32;
    }
    let text: Vec<u16> = format!(
        "Ara saved a crash report locally in:\n{}\n\nOpen the folder? Reports are never uploaded.",
        directory.display()
    )
    .encode_utf16()
    .chain([0])
    .collect();
    let title: Vec<u16> = "Ara crash report".encode_utf16().chain([0]).collect();
    // SAFETY: both strings are NUL terminated and live through the synchronous call.
    if unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x24) } == 6 {
        let _ = std::process::Command::new("explorer.exe")
            .arg(directory)
            .spawn();
    }
}

#[cfg(not(windows))]
fn offer_folder(directory: &std::path::Path) {
    eprintln!(
        "Ara saved a local crash report. Open {} to inspect it; reports are never uploaded.",
        directory.display()
    );
}
