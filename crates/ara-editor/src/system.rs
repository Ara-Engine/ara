//! Handing things to the operating system: a file or folder to its file manager or default
//! app, a terminal at the project, the engine's documentation. Nothing opens in offscreen
//! sessions (`ARA_PLAY_OFFSCREEN=1`).
use crate::Editor;

/// Open a path in the OS file manager.
pub(crate) fn open_external(path: &std::path::Path) -> ara_ui::Result<()> {
    std::fs::metadata(path)?;
    if std::env::var_os("ARA_PLAY_OFFSCREEN").is_some_and(|value| value == "1") {
        return Ok(());
    }
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(path).spawn()?;
    Ok(())
}
/// Open a terminal at the project root.
pub(crate) fn open_terminal(root: &std::path::Path) -> ara_ui::Result<()> {
    if !root.is_dir() {
        return Err("The terminal folder is no longer available".into());
    }
    if std::env::var_os("ARA_PLAY_OFFSCREEN").is_some_and(|value| value == "1") {
        return Ok(());
    }
    if cfg!(windows) {
        if std::process::Command::new("wt")
            .arg("-d")
            .arg(root)
            .spawn()
            .is_err()
        {
            std::process::Command::new("cmd")
                .args(["/c", "start", "cmd"])
                .current_dir(root)
                .spawn()?;
        }
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .args(["-a", "Terminal"])
            .arg(root)
            .spawn()?;
    } else {
        std::process::Command::new("x-terminal-emulator")
            .current_dir(root)
            .spawn()?;
    }
    Ok(())
}
/// Open the engine documentation (`docs_path`), else the project's AGENTS.md.
pub(crate) fn open_docs(editor: &Editor) -> ara_ui::Result<()> {
    let exe = std::env::current_exe()?;
    let docs = docs_path(exe.parent().unwrap_or(&exe));
    if docs.exists() {
        return open_external(&docs);
    }
    open_external(&editor.host.root().join("AGENTS.md"))
}

/// The documentation for the editor in `exe_dir`: an installed engine (its
/// `engine.json` beside the executable) has README-FIRST.txt; a checkout build has
/// the repository's `docs`. An installed editor never opens the source tree it was
/// built from, even on the machine that built it.
fn docs_path(exe_dir: &std::path::Path) -> std::path::PathBuf {
    if exe_dir.join("engine.json").is_file() {
        return exe_dir.join("README-FIRST.txt");
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs")
}

#[cfg(test)]
mod tests {
    /// An installed layout resolves to its own README, not the build's checkout.
    #[test]
    fn an_installed_editor_opens_its_own_readme() {
        let dir = std::env::temp_dir().join(format!("ara-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        std::fs::write(dir.join("engine.json"), "{}").expect("engine.json");
        assert_eq!(super::docs_path(&dir), dir.join("README-FIRST.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
