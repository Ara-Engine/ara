use super::*;
use ara_host::project::{AuthoredRole, AuthoredScope};
use ara_ui::source::stable_id;

fn click(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<bool> {
    let Some(index) = shell.shown_node(id) else {
        return Ok(false);
    };
    let node = &shell.ui.nodes[index];
    let rect = node.rect.intersection(node.clip);
    if node.state.disabled || rect.w < 2. || rect.h < node.rect.h - 0.5 {
        return Ok(false);
    }
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    if shell.ui.hit_test(point[0], point[1]) != Some(index) {
        return Ok(false);
    }
    native::motion(window, point, scale, events)?;
    native::button(window, true, events)?;
    native::button(window, false, events)?;
    Ok(true)
}

fn replace(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    value: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<bool> {
    if !click(shell, window, scale, id, events)? {
        return Ok(false);
    }
    native::shortcut(window, 4, events)?;
    native::text(window, value, events)?;
    Ok(true)
}

fn require(value: bool, message: &str) -> ara_ui::Result<()> {
    if value {
        Ok(())
    } else {
        Err(message.to_owned().into())
    }
}

impl BookmarksCheck {
    pub(super) fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let advanced = match self.phase {
            0 => return self.setup(editor, shell, window, events),
            1 => click(shell, window, scale, "assets-favourite-toggle", events)?,
            2 => {
                require(
                    editor.state.asset_favourites.get(&self.folder)
                        == Some(&crate::state::AssetFavouriteKind::Folder),
                    "Native favourite toolbar did not store the folder storage identity",
                )?;
                self.mark("favourite-added", editor, shell, json!({"favouriteKind":"folder","storageKey":self.folder,"authoredScene":self.authored}));
                true
            }
            3 => replace(shell, window, scale, "search-assets", QUERY, events)?,
            4 => {
                require(
                    editor.state.searches.get("assets").map(String::as_str) == Some(QUERY),
                    "Assets native text did not set the exact query",
                )?;
                click(shell, window, scale, "asset-search-folder-only", events)?
            }
            5 => {
                require(
                    editor.state.asset_search_folder_only,
                    "Native folder-only toggle did not enable scoped searching",
                )?;
                self.mark(
                    "scoped-query",
                    editor,
                    shell,
                    json!({"query":QUERY,"folderOnly":true}),
                );
                true
            }
            6 => {
                let field = shell
                    .ui
                    .paint_order
                    .iter()
                    .copied()
                    .find(|index| {
                        let node = &shell.ui.nodes[*index];
                        shell.ui.shown(*index)
                            && node.element.tag == "field"
                            && node
                                .element
                                .attrs
                                .get("label")
                                .is_some_and(|label| label == "Save Assets search")
                            && node
                                .element
                                .attrs
                                .get("action")
                                .is_some_and(|action| action == "ara.dialog.text")
                    })
                    .map(|index| shell.ui.nodes[index].element.id.clone());
                if let Some(field) = field {
                    require(
                        shell.input.focused() == Some(stable_id(&field)),
                        "Shared saved-search prompt did not focus its actual text field",
                    )?;
                    self.mark(
                        "save-prompt",
                        editor,
                        shell,
                        json!({"field":field,"name":SAVED}),
                    );
                    replace(shell, window, scale, &field, SAVED, events)?
                } else {
                    if !shell.modal_active() {
                        let _ = click(shell, window, scale, "assets-search-save", events)?;
                    }
                    false
                }
            }
            7 => {
                native::tap(window, 40, events)?;
                true
            }
            8 => {
                if shell.modal_active() {
                    false
                } else {
                    let saved = editor
                        .state
                        .asset_saved_searches
                        .get(SAVED)
                        .ok_or("The accepted native prompt did not create a named search")?;
                    require(
                        saved.query == QUERY && saved.folder == self.folder && saved.folder_only,
                        "Saved query lost its exact captured folder/query/scope",
                    )?;
                    self.mark(
                        "search-saved",
                        editor,
                        shell,
                        json!({"saved":saved,"name":SAVED}),
                    );
                    true
                }
            }
            9 => click(shell, window, scale, "asset-crumb-root", events)?,
            10 => {
                require(
                    editor.state.asset_folder.is_empty(),
                    "Native Project breadcrumb did not change browsing folder",
                )?;
                replace(
                    shell,
                    window,
                    scale,
                    "search-assets",
                    "type:material",
                    events,
                )?
            }
            11 => {
                require(
                    editor.state.searches.get("assets").map(String::as_str)
                        == Some("type:material"),
                    "Changed Assets query did not reach the navigation state",
                )?;
                click(shell, window, scale, "asset-search-folder-only", events)?
            }
            12 => {
                require(
                    editor.state.asset_folder.is_empty() && !editor.state.asset_search_folder_only,
                    "Changed browsing scope was not distinct from the saved search",
                )?;
                self.mark(
                    "browsing-changed",
                    editor,
                    shell,
                    json!({"distinctFolder":true,"distinctQuery":true,"distinctScope":true}),
                );
                true
            }
            13 | 17 => click(shell, window, scale, "asset-bookmark-select", events)?,
            14 => {
                let sent = replace(
                    shell,
                    window,
                    scale,
                    "choice-asset-bookmark-select-search",
                    &format!("Folder · {}", self.folder),
                    events,
                )?;
                if sent {
                    self.mark(
                        "favourite-select",
                        editor,
                        shell,
                        json!({"filter":format!("Folder · {}",self.folder)}),
                    );
                }
                sent
            }
            15 => {
                require(
                    shell.choice_open(),
                    "Favourite recall did not use an open shared Select",
                )?;
                native::tap(window, 77, events)?;
                native::tap(window, 40, events)?;
                true
            }
            16 => {
                require(
                    !shell.choice_open()
                        && editor.state.asset_folder == self.folder
                        && editor.state.asset_path.is_none(),
                    "Native Select did not recall the exact favourite folder",
                )?;
                require(
                    editor
                        .state
                        .searches
                        .get("assets")
                        .is_some_and(String::is_empty)
                        && !editor.state.asset_search_folder_only,
                    "Opening a folder favourite did not clear the query while preserving the current boolean scope",
                )?;
                self.favourite_recalled = true;
                self.mark(
                    "favourite-recalled",
                    editor,
                    shell,
                    json!({"keyboardEndEnter":true,"storageKey":self.folder}),
                );
                true
            }
            18 => {
                let sent = replace(
                    shell,
                    window,
                    scale,
                    "choice-asset-bookmark-select-search",
                    SAVED,
                    events,
                )?;
                if sent {
                    self.mark(
                        "saved-search-select",
                        editor,
                        shell,
                        json!({"filter":SAVED}),
                    );
                }
                sent
            }
            19 => {
                let label = format!("Search · {SAVED}");
                let row = shell
                    .ui
                    .paint_order
                    .iter()
                    .copied()
                    .find(|index| {
                        let node = &shell.ui.nodes[*index];
                        shell.ui.shown(*index)
                            && node.element.tag == "button"
                            && node.element.attrs.get("action").is_some_and(|action| {
                                action == "ara.select.choice-asset-bookmark-select.choose"
                            })
                            && node.text.as_ref().is_some_and(|text| text.value == label)
                    })
                    .map(|index| shell.ui.nodes[index].element.id.clone());
                if let Some(row) = row {
                    click(shell, window, scale, &row, events)?
                } else {
                    false
                }
            }
            20 => {
                require(
                    !shell.choice_open()
                        && editor.state.asset_folder == self.folder
                        && editor.state.asset_search_folder_only
                        && editor.state.searches.get("assets").map(String::as_str) == Some(QUERY),
                    "Native saved-search Select did not restore exact query + folder + folder-only scope",
                )?;
                require(
                    shell.asset_authored_files().iter().any(|file| {
                        file.storage_path == self.storage && file.path == self.authored
                    }),
                    "Bookmark recall lost the distinct storage/authored file mapping",
                )?;
                require(
                    editor.host.store().workspace().storage_path(&self.authored) == self.storage
                        && self.storage != self.authored,
                    "Saved search confused current-layout storage identity with authored-relative command paths",
                )?;
                self.search_recalled = true;
                self.mark("scoped-search-recalled", editor, shell, json!({"savedName":SAVED,"query":QUERY,"folderOnly":true,"cachedStoragePath":self.storage,"authoredPath":self.authored}));
                true
            }
            21 => {
                self.check_navigation(editor)?;
                true
            }
            _ => return Err("Bookmarks proof entered an unknown phase".into()),
        };
        if advanced {
            self.unchanged(editor)?;
            self.phase += 1;
        }
        Ok(())
    }

    fn setup(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.initial.is_none() {
            require(
                shell.offscreen && !shell.modal_active(),
                "Bookmarks proof requires an idle offscreen shell",
            )?;
            self.started = Some(Instant::now());
            self.initial = Some(Initial {
                folder: editor.state.asset_folder.clone(),
                query: editor
                    .state
                    .searches
                    .get("assets")
                    .cloned()
                    .unwrap_or_default(),
                scope: editor.state.asset_search_folder_only,
                asset: editor.state.asset_path.clone(),
                maximized: editor.state.maximized.clone(),
                focus: editor.state.focus.clone(),
            });
            shell.leave();
            shell.menu = None;
            shell.desktop_pointer = None;
            editor.action(
                "editor.layout.move",
                json!({"panel":"assets","slot":"view"}),
            )?;
            editor.action("editor.view", json!({"maximized":"assets"}))?;
            editor.action("editor.focus", json!({"panel":"assets"}))?;
            shell.refresh_asset_browser()?;
            shell.rebuild(editor)?;
            native::focus_lost(window, events)?;
        }
        let file = shell.asset_authored_files().iter().find(|file| {
            matches!(file.scope, AuthoredScope::Authored)
                && matches!(&file.role,AuthoredRole::Document{name} if name=="scene")
        });
        let Some(file) = file else {
            return Ok(());
        };
        self.authored = file.path.clone();
        self.storage = file.storage_path.clone();
        require(
            editor.host.store().workspace().storage_path(&self.authored) == self.storage
                && self.storage != self.authored,
            "The semantic index did not preserve the current project's distinct authored and storage paths",
        )?;
        self.folder = self
            .storage
            .rsplit_once('/')
            .map(|(folder, _)| folder)
            .ok_or("Indexed scene has no containing storage folder")?
            .to_owned();
        require(
            !editor.state.asset_favourites.contains_key(&self.folder)
                && !editor.state.asset_saved_searches.contains_key(SAVED),
            "The isolated bookmarks proof found a stale entry using its own keys",
        )?;
        self.owns_keys = true;
        editor.action(
            "editor.workspace",
            json!({"folder":self.folder,"asset":"","assetQuery":"","folderOnly":false}),
        )?;
        shell.rebuild(editor)?;
        self.revision = editor.host.store().revision();
        self.documents = proof::documents(editor);
        self.doc = editor.state.doc.clone();
        self.camera = Some(editor.state.camera);
        self.selection = editor.state.selection.clone();
        self.nav_start = proof::history(editor)?.len();
        self.mark("before",editor,shell,json!({"setup":"typed navigation only","existingScene":self.authored,"storagePath":self.storage,"folderStorageKey":self.folder}));
        self.phase = 1;
        Ok(())
    }
}
