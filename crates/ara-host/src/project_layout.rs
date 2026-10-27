//! Shared project scaffold and offline layout migration, used by the CLI and Hub.
mod mapping;
mod migrate;
mod references;

pub use ara_doc::AUTHORING_ROOT;
pub use migrate::{Migration, MigrationOp};

use std::{fs, io, path::Path};

/// The freely organized root for authored scenes, source media and other content.
pub const CONTENT: &str = AUTHORING_ROOT;
/// The starter scene in newly created projects; existing projects may choose any path.
pub const MAIN_SCENE: &str = "scenes/main.scene";
/// Authored project configuration and store-agnostic export presets.
pub const SETTINGS: &str = "settings";
/// Flat canonical policy document, authored through ordinary `property.set` commands.
pub const PREFERENCES: &str = "settings/project.json";
/// Synthetic identity of the policy document in the host workspace.
pub const PREFERENCES_ID: &str = "pj_preferences";
pub(crate) const POLICY_KEYS: &[&str] = &[
    "settings",
    "rendering",
    "input",
    "physics",
    "look",
    "layers",
    "ui",
];
/// Export output; ignored by git and project discovery.
pub const BUILDS: &str = "builds";
/// The current on-disk layout version in `ara.project.json`.
pub const VERSION: u64 = 3;
/// Instructions installed at the project root for humans and coding agents.
pub const AGENT_GUIDE: &str = include_str!("project_layout/agents.md");

/// Creates the common directories without changing existing authored files.
pub fn create_directories(root: &Path) -> io::Result<()> {
    for directory in [
        format!("{AUTHORING_ROOT}/scenes"),
        format!("{AUTHORING_ROOT}/editor"),
        "settings/builds".to_owned(),
        BUILDS.to_owned(),
        "packages".to_owned(),
    ] {
        fs::create_dir_all(root.join(directory))?;
    }
    Ok(())
}

/// Physical authored root; legacy projects remain readable until migrated.
/// Cache this result when opening a project rather than calling it in frame code.
pub use crate::project::authored_root;

/// Root support files shared by CLI creation and Hub templates; existing files stay intact.
pub fn write_support_files(root: &Path, name: &str) -> io::Result<()> {
    create_directories(root)?;
    for (path, text) in [
        (
            "AGENTS.md",
            AGENT_GUIDE
                .replace("{name}", name)
                .replace("{authoring_root}", AUTHORING_ROOT),
        ),
        (
            ".mcp.json",
            ara_json::to_canonical_string(
                &serde_json::json!({"mcpServers":{"ara":{"command":"ara","args":["mcp","--root","."]}}}),
            ),
        ),
        (
            ".gitattributes",
            "*.scene merge=ara\n*.prefab merge=ara\n* text=auto eol=lf\n".to_owned(),
        ),
        (
            "settings/builds/windows.export.json",
            ara_json::to_canonical_string(&default_build_preset(name)),
        ),
        (PREFERENCES, "{}\n".to_owned()),
    ] {
        let path = root.join(path);
        if !path.exists() {
            fs::write(path, text)?;
        }
    }
    let path = root.join(".gitignore");
    let mut text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    for line in [".ara/", "/builds/"] {
        if !text.lines().any(|old| old.trim() == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
        }
    }
    fs::write(path, text)
}

/// Reads project policy, preferring the dedicated document over legacy marker fields.
/// The result retains the existing settings, rendering, input, physics, look and
/// layers and UI containers; omitted values continue to inherit engine defaults.
pub fn preferences(root: &Path) -> Result<serde_json::Value, ara_diag::Diagnostic> {
    let marker = read_object(&root.join(crate::project::MARKER))?;
    let mut preferences = serde_json::Map::new();
    for key in POLICY_KEYS {
        if let Some(value) = marker.get(*key) {
            preferences.insert((*key).to_owned(), value.clone());
        }
    }
    if let serde_json::Value::Object(saved) = read_object(&root.join(PREFERENCES))? {
        preferences.extend(saved);
    }
    Ok(serde_json::Value::Object(preferences))
}

fn read_object(path: &Path) -> Result<serde_json::Value, ara_diag::Diagnostic> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(serde_json::json!({})),
        Err(e) => {
            return Err(
                ara_diag::Diagnostic::error(ara_diag::codes::IO, e.to_string())
                    .file(path.display().to_string()),
            );
        }
    };
    let value = ara_json::parse(&text).map_err(|e| {
        ara_diag::Diagnostic::error(ara_diag::codes::JSON_SYNTAX, e.message)
            .file(path.display().to_string())
            .line_col(e.line, e.column)
    })?;
    if !value.is_object() {
        return Err(ara_diag::Diagnostic::error(
            ara_diag::codes::INVALID_PARAMS,
            "project policy must be a JSON object",
        )
        .file(path.display().to_string()));
    }
    Ok(value)
}

/// Reads a policy container from the current transactional workspace, before disk saves.
pub fn policy<'a>(workspace: &'a ara_doc::Workspace, key: &str) -> Option<&'a serde_json::Value> {
    [PREFERENCES_ID, "pj_project"].into_iter().find_map(|id| {
        workspace
            .doc(id)
            .and_then(|doc| doc.record("p_settings"))
            .and_then(|record| record.get("settings"))
            .and_then(|settings| settings.get(key))
    })
}

/// Initial store-agnostic desktop preset; runtime pins are supplied by an engine release.
pub fn default_build_preset(assembly: &str) -> serde_json::Value {
    let assembly: String = assembly
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let assembly = if assembly.is_empty() {
        "Game"
    } else {
        &assembly
    };
    serde_json::json!({
        "$schema": "ara://schema/export/1", "platform": "windows", "config": "release",
        "lane": "nativeAot", "playerTemplate": "runtime", "assembly": assembly,
        "content": CONTENT, "templates": []
    })
}
