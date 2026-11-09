//! Operating-system effects: folder pickers, Explorer and the browser.
use crate::model::Pick;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Shows the system folder picker (modal) and returns the chosen folder.
pub fn pick_folder(pick: Pick, start: Option<&Path>) -> Option<PathBuf> {
    let title = match pick {
        Pick::Register => "Add an existing project",
        Pick::Destination => "Choose where to create the project",
        Pick::Engine => "Add an engine build",
    };
    stillkit::dialog::pick_folder(title, start)
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
