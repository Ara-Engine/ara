//! Native toolbar/sidebar affordances share the same typed bookmark operations.
use super::*;

impl Shell {
    pub(in crate::shell) fn asset_favourites_toolbar(&mut self, editor: &Editor, out: &mut String) {
        let (path, _) = self.favourite_target(editor, &Target::None);
        let saved = editor.state.asset_favourites.contains_key(&path);
        self.bind_command(
            "assets-favourite-toggle",
            "assets.favourite.toggle",
            Target::None,
        );
        self.bind_command("assets-search-save", "assets.search.save", Target::None);
        out.push_str(&format!("<button id='assets-favourite-toggle' class='{} icon-button' label='{} favourite' description='Toggle a favourite for the inspected asset or current folder. Favourites are saved with this project layout.' action='Dispatch'>{}</button><button id='assets-search-save' class='ghost icon-button' label='Save Assets search' description='Save the query, type filters and folder search scope by name' action='Dispatch'>{}</button>", if saved {"selected"} else {"ghost"}, if saved {"Remove"} else {"Add"}, icons::icon("assets-favourite-toggle","star"), icons::icon("assets-search-save","bookmark")));
        if self.assets_favourites.removed.is_some() {
            self.bind_command(
                "assets-search-restore",
                "assets.search.restore",
                Target::None,
            );
            out.push_str(&format!("<button id='assets-search-restore' class='ghost icon-button' label='Restore removed Assets search' description='Restore only the last removed saved search; keep the current view and later bookmarks' action='Dispatch'>{}</button>", icons::icon("assets-search-restore","undo-2")));
        }
    }
    /// The same searchable Select is mounted alongside Type at every browser width.
    pub(in crate::shell) fn asset_bookmarks_select(&mut self, editor: &Editor, out: &mut String) {
        let mut options = vec![(Value::Null, "Bookmarks".into())];
        if matches!(self.assets_favourites.pick, Pick::All | Pick::Favourites) {
            options.extend(editor.state.asset_favourites.iter().map(|(path, kind)| {
                let label = if path.is_empty() {
                    "Project"
                } else {
                    path.as_str()
                };
                (
                    json!({"favourite":path}),
                    format!(
                        "{} · {label}",
                        if *kind == AssetFavouriteKind::Folder {
                            "Folder"
                        } else {
                            "Asset"
                        }
                    ),
                )
            }));
        }
        if matches!(
            self.assets_favourites.pick,
            Pick::All | Pick::Searches | Pick::RemoveSearch
        ) {
            options.extend(editor.state.asset_saved_searches.keys().map(|name| {
                (
                    if self.assets_favourites.pick == Pick::RemoveSearch {
                        json!({"removeSearch":name})
                    } else {
                        json!({"search":name})
                    },
                    format!(
                        "{} · {name}",
                        if self.assets_favourites.pick == Pick::RemoveSearch {
                            "Remove search"
                        } else {
                            "Search"
                        }
                    ),
                )
            }));
        }
        out.push_str("<panel id='asset-bookmark-value' style='flex-basis:0px;flex-grow:1;min-width:0px;flex-shrink:1'>");
        self.choice_button(
            "asset-bookmark-select",
            "Favourites and saved Assets searches",
            choices::Select {
                destination: choices::Destination::AssetBookmark,
                options,
                current: Value::Null,
            },
            out,
        );
        self.bind_command(
            "asset-search-folder-only",
            "assets.search.scope",
            Target::None,
        );
        out.push_str(&format!("</panel><toggle id='asset-search-folder-only' checked='{}' style='width:22px;min-width:22px;flex-shrink:0' label='Search current folder only' description='Checked: queries and type filters search the current folder and descendants. Unchecked: search the entire project.' action='Dispatch' />", editor.state.asset_search_folder_only));
    }
    pub(in crate::shell) fn asset_bookmarks_sidebar(&mut self, editor: &Editor, out: &mut String) {
        if !editor.state.asset_favourites.is_empty() {
            out.push_str("<text id='asset-favourites-heading' class='caption' style='padding:4px;flex-shrink:0' text='Favourites' />");
        }
        for (path, kind) in &editor.state.asset_favourites {
            let id = format!("asset-favourite-{:016x}", ara_ui::source::stable_id(path));
            let target = if *kind == AssetFavouriteKind::Folder {
                Target::Folder(path.clone())
            } else {
                Target::Asset(path.clone())
            };
            self.bind_command(&id, "assets.favourite.open", target.clone());
            self.bind_command(&format!("{id}-remove"), "assets.favourite.toggle", target);
            let label = if path.is_empty() {
                "Project"
            } else {
                names::file_name(path)
            };
            out.push_str(&format!("<panel id='{id}-row' class='row' style='flex-shrink:0;gap:0px;min-width:0px'><button id='{id}' class='folder-row' style='flex-grow:1;min-width:0px' label='Open favourite {}' description='{}' action='Dispatch'>{}<text id='{id}-label' text='{}' /></button><button id='{id}-remove' class='ghost icon-button' label='Remove favourite {}' action='Dispatch'>{}</button></panel>",escape(path),escape(path),icons::icon(&id,if *kind==AssetFavouriteKind::Folder {"folder"} else {"file"}),escape(label),escape(path),icons::icon(&format!("{id}-remove"),"x")));
        }
        if !editor.state.asset_saved_searches.is_empty() {
            out.push_str("<text id='asset-saved-searches-heading' class='caption' style='padding:4px;flex-shrink:0' text='Saved searches' />");
        }
        for (name, search) in &editor.state.asset_saved_searches {
            let id = format!(
                "asset-saved-search-{:016x}",
                ara_ui::source::stable_id(name)
            );
            self.bind_command(&id, "assets.search.apply", Target::Text(name.clone()));
            self.bind_command(
                &format!("{id}-remove"),
                "assets.search.remove",
                Target::Text(name.clone()),
            );
            let description = format!(
                "{} · {} · {}",
                if search.folder_only {
                    "Folder and descendants"
                } else {
                    "Whole project"
                },
                if search.folder.is_empty() {
                    "Project"
                } else {
                    &search.folder
                },
                search.query
            );
            out.push_str(&format!("<panel id='{id}-row' class='row' style='flex-shrink:0;gap:0px;min-width:0px'><button id='{id}' class='folder-row' style='flex-grow:1;min-width:0px' label='Apply saved search {}' description='{}' action='Dispatch'>{}<text id='{id}-label' text='{}' /></button><button id='{id}-remove' class='ghost icon-button' label='Remove saved search {}' action='Dispatch'>{}</button></panel>",escape(name),escape(&description),icons::icon(&id,"search"),escape(name),escape(name),icons::icon(&format!("{id}-remove"),"x")));
        }
    }
}
