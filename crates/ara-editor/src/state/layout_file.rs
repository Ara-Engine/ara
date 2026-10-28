//! Versioned editor layout files; old arrangements reset once to the new baseline.
use super::{AssetFavouriteKind, EditorState, SavedAssetSearch, default_dock};
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
        &json!({"version": VERSION, "layout": layout, "assets": {
            "sidebarWidth": state.asset_sidebar_width,
            "expandedFolders": state.asset_expanded_folders,
            "favourites": state.asset_favourites,
            "savedSearches": state.asset_saved_searches,
            "searchFolderOnly": state.asset_search_folder_only
        }}),
    ))
}

/// Optional bookmark fields extend layout version 2 without resetting old layouts.
pub(super) fn restore_asset_bookmarks(saved: &serde_json::Value, state: &mut EditorState) {
    if let Some(favourites) = saved["assets"]["favourites"].as_object() {
        state.asset_favourites = favourites
            .iter()
            .filter_map(|(path, kind)| {
                let kind: AssetFavouriteKind = serde_json::from_value(kind.clone()).ok()?;
                super::valid_asset_bookmark_path(path, kind == AssetFavouriteKind::Folder)
                    .then(|| (path.clone(), kind))
            })
            .collect();
    }
    if let Some(searches) = saved["assets"]["savedSearches"].as_object() {
        state.asset_saved_searches = searches
            .iter()
            .filter_map(|(name, value)| {
                let search: SavedAssetSearch = serde_json::from_value(value.clone()).ok()?;
                (super::valid_asset_search_name(name)
                    && super::valid_asset_bookmark_path(&search.folder, true)
                    && search.query.chars().count() <= 4096)
                    .then(|| (name.clone(), search))
            })
            .collect();
    }
    if let Some(folder_only) = saved["assets"]["searchFolderOnly"].as_bool() {
        state.asset_search_folder_only = folder_only;
    }
}
