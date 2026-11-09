//! Operating-system effects: folder pickers, Explorer and the browser.
use crate::model::Pick;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        mpsc::{Receiver, Sender, channel},
    },
};

static PICKED: Mutex<Option<Sender<(Pick, PathBuf)>>> = Mutex::new(None);

/// Folder-picker answers, delivered by SDL on any thread.
pub fn picks() -> Receiver<(Pick, PathBuf)> {
    let (send, receive) = channel();
    if let Ok(mut slot) = PICKED.lock() {
        *slot = Some(send);
    }
    receive
}

unsafe extern "C" fn picked(
    user: *mut std::ffi::c_void,
    files: *const *const std::ffi::c_char,
    _: std::ffi::c_int,
) {
    if files.is_null() {
        return;
    }
    let pick = match user as usize {
        0 => Pick::Register,
        1 => Pick::Destination,
        _ => Pick::Engine,
    };
    // SAFETY: SDL passes a null-terminated array of UTF-8 paths valid for this call.
    let first = unsafe { *files };
    if first.is_null() {
        return;
    }
    // SAFETY: Non-null entries are nul-terminated strings owned by SDL for this call.
    let path = unsafe { std::ffi::CStr::from_ptr(first) }
        .to_string_lossy()
        .into_owned();
    if let Ok(slot) = PICKED.lock()
        && let Some(send) = slot.as_ref()
    {
        let _ = send.send((pick, PathBuf::from(path)));
        super::wake::wake();
    }
}

/// Show the OS folder picker; the answer arrives on [`picks`].
pub fn pick_folder(window: *mut sdl3_sys::video::SDL_Window, pick: Pick, start: Option<&Path>) {
    let user = match pick {
        Pick::Register => 0usize,
        Pick::Destination => 1,
        Pick::Engine => 2,
    };
    let start = start.and_then(|p| std::ffi::CString::new(crate::model::text::path(p)).ok());
    // SAFETY: The window is live; SDL copies the default location and calls back with
    // the encoded purpose as opaque userdata (never dereferenced).
    unsafe {
        sdl3_sys::dialog::SDL_ShowOpenFolderDialog(
            Some(picked),
            user as *mut std::ffi::c_void,
            window,
            start.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            false,
        );
    }
}

/// Select `path` in Explorer (Finder, or the file manager elsewhere).
pub fn reveal(path: &Path) {
    #[cfg(any(windows, target_os = "macos"))]
    let shown = crate::model::text::path(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{shown}\""))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("-R").arg(&shown).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
}

/// Open a file or folder with its default application.
pub fn open(path: &Path) {
    let shown = crate::model::text::path(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("explorer.exe")
            .raw_arg(format!("\"{shown}\""))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(&shown).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(&shown).spawn();
}

/// Open a web page in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("explorer.exe")
            .raw_arg(format!("\"{url}\""))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}
