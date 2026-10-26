//! Versioned editor layout files; old arrangements reset once to the new baseline.
use super::{EditorState, default_dock};
use ara_ui::docking::DockLayout;
use serde_json::json;

/// Editor layout format. Version 1 was an unversioned DockNode/Saved payload.
pub const VERSION: u64 = 2;
pub(crate) const MIGRATION_NOTICE: &str = "Saved layout reset for the new editor layout.";
pub(crate) fn restore(text: &str) -> ara_ui::Result<(DockLayout, bool)> {
    let value: serde_json::Value = ara_json::parse(text)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    if version < VERSION {
        return Ok((default_dock()?, true));
    }
    if version != VERSION {
        return Err(format!("Unsupported editor layout version {version}").into());
    }
    let layout = value.get("layout").ok_or("Editor layout payload missing")?;
    let mut dock = default_dock()?;
    dock.restore(&ara_json::to_canonical_string(layout))?;
    Ok((dock, false))
}
pub(crate) fn serialize(state: &EditorState) -> anyhow::Result<String> {
    let text = state
        .dock()
        .and_then(|dock| dock.serialize())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let layout: serde_json::Value = ara_json::parse(&text)?;
    Ok(ara_json::to_canonical_string(
        &json!({"version": VERSION, "layout": layout}),
    ))
}
