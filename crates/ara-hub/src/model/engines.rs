//! Finding engine builds on disk: the one this Hub ships with, and folders the
//! user picks in Installs.
use super::wire::Engine;
use std::path::Path;

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// The installed distribution next to `hub_dir` (it has an `engine.json`), which the
/// Hub registers on first run. Development builds have no manifest and return None;
/// the service registers its own executable when it starts.
pub fn installed(hub_dir: &Path) -> Option<Engine> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(hub_dir.join("engine.json")).ok()?).ok()?;
    if manifest["format"] != "ara.engine/1" {
        return None;
    }
    let executable = hub_dir.join(exe("ara"));
    let editor = hub_dir.join(exe("ara-editor"));
    executable.is_file().then(|| Engine {
        version: manifest["version"].as_str().unwrap_or_default().to_owned(),
        executable,
        editor: editor.is_file().then_some(editor),
    })
}

/// An engine build in a user-picked folder: its `engine.json` version, else the
/// version `ara --version` prints.
pub fn probe(folder: &Path) -> Result<Engine, String> {
    if let Some(engine) = installed(folder) {
        return Ok(engine);
    }
    let executable = folder.join(exe("ara"));
    if !executable.is_file() {
        return Err(format!(
            "`{}` has no {}; choose the folder of an Ara installation",
            super::text::path(folder),
            exe("ara")
        ));
    }
    let output = std::process::Command::new(&executable)
        .arg("--version")
        .output()
        .map_err(|e| format!("Cannot run {}: {e}", super::text::path(&executable)))?;
    let version = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
        .filter(|v| output.status.success() && !v.is_empty())
        .ok_or_else(|| "`ara --version` did not report a version".to_owned())?;
    let editor = folder.join(exe("ara-editor"));
    Ok(Engine {
        version,
        executable,
        editor: editor.is_file().then_some(editor),
    })
}
