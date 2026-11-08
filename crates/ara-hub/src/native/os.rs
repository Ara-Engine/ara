//! Operating-system effects: folder pickers, Explorer, the install snippets
//! printed by `ara hub install-mcp`, and the docs present on this machine.
use crate::{
    model::{AgentClient, Pick},
    ui::Doc,
};
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
