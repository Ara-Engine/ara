//! Project-local browser bookmarks use typed navigation, separate from content Undo.
use super::*;
use crate::state::{AssetFavouriteKind, SavedAssetSearch};
use ara_ui::dialogs::{DialogFuture, DialogOptions};
use commands::Target;
#[cfg(test)]
mod tests;
mod view;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Pick {
    #[default]
    All,
    Favourites,
    Searches,
    RemoveSearch,
}
struct Naming {
    search: SavedAssetSearch,
    response: DialogFuture<Option<String>>,
}
/// Transient naming and scoped restore state; persisted entries live in EditorState.
#[derive(Default)]
pub(crate) struct View {
    naming: Option<Naming>,
    removed: Option<(String, SavedAssetSearch)>,
    pick: Pick,
    /// Changes when picker/restore presentation changes without a navigation edit.
    pub(super) revision: u64,
}

/// A scoped query searches descendants, while an empty query keeps ordinary
/// immediate folder browsing. Type tokens remain part of the existing query.
pub(super) fn in_scope(path: &str, folder: &str, searching: bool, folder_only: bool) -> bool {
    let relative = if folder.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(folder)
            .and_then(|tail| tail.strip_prefix('/'))
    };
    if searching {
        !folder_only || relative.is_some()
    } else {
        relative.is_some_and(|tail| !tail.contains('/'))
    }
}

impl Shell {
    fn current_asset_search(&self, editor: &Editor) -> SavedAssetSearch {
        SavedAssetSearch {
            query: editor
                .state
                .searches
                .get("assets")
                .cloned()
                .unwrap_or_default(),
            folder: editor.state.asset_folder.clone(),
            folder_only: editor.state.asset_search_folder_only,
        }
    }
    fn favourite_target(&self, editor: &Editor, target: &Target) -> (String, AssetFavouriteKind) {
        match target {
            Target::Folder(path) => (path.clone(), AssetFavouriteKind::Folder),
            Target::Asset(path) | Target::Text(path) => {
                let kind = editor
                    .state
                    .asset_favourites
                    .get(path)
                    .copied()
                    .unwrap_or_else(|| {
                        if self
                            .browser
                            .items
                            .iter()
                            .any(|asset| asset.path == *path && asset.kind == "folder")
                        {
                            AssetFavouriteKind::Folder
                        } else {
                            AssetFavouriteKind::Asset
                        }
                    });
                (path.clone(), kind)
            }
            _ => editor.state.asset_path.as_ref().map_or_else(
                || {
                    (
                        editor.state.asset_folder.clone(),
                        AssetFavouriteKind::Folder,
                    )
                },
                |path| (path.clone(), AssetFavouriteKind::Asset),
            ),
        }
    }
    /// Reverse a favourite with one explicit navigation transaction.
    pub(crate) fn toggle_asset_favourite(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let (path, kind) = self.favourite_target(editor, target);
        editor.action(
            "editor.workspace",
            json!({"favourite":path,"favouriteFolder":kind==AssetFavouriteKind::Folder}),
        )?;
        self.rebuild(editor)
    }
    /// Open a stored storage-path bookmark, or expose the searchable picker.
    pub(crate) fn open_asset_favourite(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let path = match target {
            Target::Asset(path) | Target::Folder(path) | Target::Text(path) => path,
            _ => return self.open_asset_bookmark_picker(editor, Pick::Favourites),
        };
        if !editor.state.asset_favourites.contains_key(path) {
            return Err("That favourite no longer exists; choose a current bookmark".into());
        }
        if !path.is_empty()
            && !self.browser.items.iter().any(|asset| asset.path == *path)
            && path != editor.host.store().workspace().reference_root()
        {
            return Err("This favourite is missing from the browser index. Refresh Assets to retry, or remove the favourite.".into());
        }
        editor.action("editor.workspace", json!({"openFavourite":path}))?;
        self.rebuild(editor)
    }
    /// Capture query, type tokens and folder scope before opening the naming prompt.
    pub(crate) fn save_asset_search(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let search = self.current_asset_search(editor);
        if let Target::Text(name) = target {
            editor.action(
                "editor.workspace",
                json!({"saveSearch":name,"savedSearch":search}),
            )?;
            return self.rebuild(editor);
        }
        if self.dialog.is_some()
            || self.confirmations.is_open()
            || self.assets_favourites.naming.is_some()
            || self.modal_root == Some("build-options")
        {
            return Err(
                "Finish or cancel the current dialog before naming an Assets search".into(),
            );
        }
        if editor.state.palette {
            editor.action("editor.palette", json!({"open":false}))?;
        }
        self.menu = None;
        let scope = if search.folder_only {
            "this folder and its descendants"
        } else {
            "the whole project"
        };
        let response = self.confirmations.prompt(DialogOptions::new("Save Assets search", format!("Save the query and type filters for {scope}. An existing name updates that entry.\nFolder: {}\nQuery: {}", if search.folder.is_empty() { "Project" } else { &search.folder }, search.query)), String::new());
        self.assets_favourites.naming = Some(Naming { search, response });
        self.rebuild(editor)
    }
    /// Apply/remove one named query. Removal keeps the current view and offers a
    /// scoped Restore operation; ordinary content Undo is never repurposed.
    pub(crate) fn use_asset_search(
        &mut self,
        editor: &mut Editor,
        target: &Target,
        remove: bool,
    ) -> ara_ui::Result<()> {
        let Target::Text(name) = target else {
            return self.open_asset_bookmark_picker(
                editor,
                if remove {
                    Pick::RemoveSearch
                } else {
                    Pick::Searches
                },
            );
        };
        if name.is_empty() {
            return Ok(());
        }
        if remove {
            let saved = editor
                .state
                .asset_saved_searches
                .get(name)
                .cloned()
                .ok_or("That saved Assets search no longer exists")?;
            editor.action("editor.workspace", json!({"removeSearch":name}))?;
            self.assets_favourites.removed = Some((name.clone(), saved));
        } else {
            editor.action("editor.workspace", json!({"openSearch":name}))?;
        }
        self.assets_favourites.pick = Pick::All;
        self.assets_favourites.revision += 1;
        self.rebuild(editor)
    }
    /// Restore only the last explicitly removed entry, preserving later searches,
    /// current query/folder, scene selection, camera and content history.
    pub(crate) fn restore_asset_search(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let (name, saved) = self
            .assets_favourites
            .removed
            .as_ref()
            .ok_or("There is no removed Assets search to restore")?;
        if editor.state.asset_saved_searches.contains_key(name) {
            return Err("A saved search now uses that name. Keep it, or remove it before restoring the older entry.".into());
        }
        editor.action(
            "editor.workspace",
            json!({"saveSearch":name,"savedSearch":saved}),
        )?;
        self.assets_favourites.removed = None;
        self.assets_favourites.revision += 1;
        self.rebuild(editor)
    }
    /// Toggle the boolean query scope, or accept an explicit `folder` / `project` target.
    pub(crate) fn set_asset_search_scope(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let folder_only = match target {
            Target::Text(value) if value == "folder" => true,
            Target::Text(value) if value == "project" => false,
            Target::Text(_) => return Err("Choose folder or project search scope".into()),
            _ => !editor.state.asset_search_folder_only,
        };
        editor.action("editor.workspace", json!({"folderOnly":folder_only}))?;
        self.rebuild(editor)
    }
    fn open_asset_bookmark_picker(
        &mut self,
        editor: &mut Editor,
        pick: Pick,
    ) -> ara_ui::Result<()> {
        if self.dialog.is_some()
            || self.confirmations.is_open()
            || self.modal_root == Some("build-options")
        {
            return Err(
                "Finish or cancel the current dialog before opening browser bookmarks".into(),
            );
        }
        let empty = match pick {
            Pick::Favourites => editor.state.asset_favourites.is_empty(),
            Pick::Searches | Pick::RemoveSearch => editor.state.asset_saved_searches.is_empty(),
            Pick::All => {
                editor.state.asset_favourites.is_empty()
                    && editor.state.asset_saved_searches.is_empty()
            }
        };
        if empty {
            return Err(
                "Save a favourite or named Assets search before opening this picker".into(),
            );
        }
        if editor.state.palette {
            editor.action("editor.palette", json!({"open":false}))?;
        }
        self.menu = None;
        self.assets_favourites.pick = pick;
        self.assets_favourites.revision += 1;
        editor.action("editor.layout.show", json!({"panel":"assets"}))?;
        editor.action("editor.focus", json!({"panel":"assets"}))?;
        self.rebuild(editor)?;
        self.open_choice(editor, "asset-bookmark-select")
    }
    /// Accept a typed shared-Select intent; names and storage paths stay distinct.
    pub(in crate::shell) fn choose_asset_bookmark(
        &mut self,
        editor: &mut Editor,
        value: Value,
    ) -> ara_ui::Result<()> {
        if value.is_null() {
            return Ok(());
        }
        if let Some(path) = value.get("favourite").and_then(Value::as_str) {
            self.assets_favourites.pick = Pick::All;
            self.assets_favourites.revision += 1;
            self.open_asset_favourite(editor, &Target::Asset(path.into()))
        } else if let Some(name) = value.get("search").and_then(Value::as_str) {
            self.use_asset_search(editor, &Target::Text(name.into()), false)
        } else if let Some(name) = value.get("removeSearch").and_then(Value::as_str) {
            self.use_asset_search(editor, &Target::Text(name.into()), true)
        } else {
            Err("Choose a current favourite or saved Assets search".into())
        }
    }
    /// Poll a shared prompt without applying a background query to the active view.
    pub(crate) fn poll_asset_favourites(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let reset = self.assets_favourites.pick != Pick::All && !self.choice_open();
        if reset {
            self.assets_favourites.pick = Pick::All;
            self.assets_favourites.revision += 1;
        }
        let reply = self
            .assets_favourites
            .naming
            .as_mut()
            .and_then(|pending| pending.response.try_take());
        let Some(reply) = reply else {
            if reset {
                self.rebuild(editor)?;
            }
            return Ok(reset);
        };
        let pending = self
            .assets_favourites
            .naming
            .take()
            .ok_or("Assets search prompt was removed")?;
        if let Some(name) = reply {
            editor.action(
                "editor.workspace",
                json!({"saveSearch":name,"savedSearch":pending.search}),
            )?;
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}
