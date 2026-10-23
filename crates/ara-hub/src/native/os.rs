//! Operating-system effects: folder pickers, Explorer, the install snippets
//! printed by `ara hub install-mcp`, and the docs present on this machine.
use crate::{
    model::{AgentClient, Pick},
    view::Doc,
};
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

fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: console tools never flash a window from the Hub.
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Select `path` in Explorer (Finder, or the file manager elsewhere).
pub fn reveal(path: &Path) {
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

/// `ara hub install-mcp --client …` output for each client, printed by the real CLI.
pub fn install_snippets(ara: &Path) -> Vec<(AgentClient, String)> {
    [AgentClient::Claude, AgentClient::Codex]
        .into_iter()
        .filter_map(|client| {
            let output =
                hidden(Command::new(ara).args(["hub", "install-mcp", "--client", client.id()]))
                    .output()
                    .ok()?;
            output
                .status
                .success()
                .then(|| (client, String::from_utf8_lossy(&output.stdout).into_owned()))
        })
        .collect()
}

/// Docs that exist next to this build: the installed README, or a source checkout's.
pub fn docs(exe_dir: &Path) -> Vec<Doc> {
    let doc = |title: &str, caption: &str, path: PathBuf| {
        path.is_file().then(|| Doc {
            title: title.into(),
            caption: caption.into(),
            path,
        })
    };
    let mut docs: Vec<Doc> = [doc(
        "Read me first",
        "Requirements, installation and first commands for this build",
        exe_dir.join("README-FIRST.txt"),
    )]
    .into_iter()
    .flatten()
    .collect();
    if let Some(root) = exe_dir
        .ancestors()
        .find(|dir| dir.join("docs/decisions.md").is_file())
    {
        docs.extend(
            [
                doc(
                    "Ara README",
                    "What Ara is and how to build it",
                    root.join("README.md"),
                ),
                doc(
                    "Samples guide",
                    "Outpost and Showcase: fetching art, building and capturing",
                    root.join("samples/README.md"),
                ),
                doc(
                    "Hub and global MCP",
                    "Projects, lifecycle and agent setup reference",
                    root.join("crates/ara-hub-service/AGENTS.md"),
                ),
                doc(
                    "Known issues",
                    "What does not work yet in this alpha",
                    root.join("docs/KNOWN_ISSUES.md"),
                ),
                doc(
                    "Engine decisions",
                    "The locked design decisions behind Ara",
                    root.join("docs/decisions.md"),
                ),
            ]
            .into_iter()
            .flatten(),
        );
    }
    docs
}
