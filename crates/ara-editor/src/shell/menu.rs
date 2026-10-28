//! One renderer for menu bar menus, context menus and value menus. Every row
//! runs a registry command; disabled rows keep their reason as a tooltip.
use super::commands::{
    Avail, Target,
    menus::{self, Context, Dynamic, Item, MENUS},
};
use super::*;

/// Where an open menu's rows come from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    /// A top-level menu by its registered title, independent of catalog order.
    Bar(String),
    /// A right-click (or ⋯) menu for a target.
    Context(Context, Target),
    /// Snap increments (`move`, `rotate`, `scale`) or Scene view fields of view (`fov`).
    Values(&'static str),
    /// An inspector asset field's picker: assets that fit it, and None.
    Assets(String, Option<String>),
}
/// An open menu: its root anchor, open submenus and keyboard highlight.
#[derive(Clone, Debug)]
pub(crate) struct MenuState {
    pub source: Source,
    /// The control the menu opened from (a bar title, a dropdown button, a field); the
    /// root popover is placed against it. Without one it opens at `at`.
    pub anchor: Option<String>,
    /// Root popover point when there is no anchor (a right-click), in the surface's
    /// logical pixels.
    pub at: [f32; 2],
    /// Surface that shows the menu; `None` is the main window.
    pub surface: Option<String>,
    /// Open submenu title per level, independent of sibling order.
    pub open: Vec<String>,
    /// Keyboard highlight in the deepest level.
    pub active: Option<usize>,
    /// Type-to-filter text (root level).
    pub query: String,
    /// Last painted row identities, used to preserve a keyboard highlight on reorder.
    painted_rows: Vec<String>,
    /// Effective authoring/navigation target fingerprint, calculated once per rebuild.
    context_identity: String,
}
impl MenuState {
    pub fn bar(&self) -> Option<&str> {
        match &self.source {
            Source::Bar(title) => Some(title),
            _ => None,
        }
    }
}
/// A resolved row.
#[derive(Clone, Debug)]
pub(crate) enum Row {
    Command {
        id: String,
        target: Target,
        /// Title override (dynamic and value rows) or a filter-result path.
        label: Option<String>,
        path: String,
        checked: Option<bool>,
    },
    Sub(String, Vec<Row>),
    Sep,
}
const WIDTH: f32 = 144.;

/// A header press cannot change roots when registered titles reorder or reload.
pub(crate) fn bar_id(title: &str, generation: Option<&str>) -> String {
    let value = ara_json::to_canonical_string(&json!([title, generation]));
    format!("menu-title-{}", blake3::hash(value.as_bytes()).to_hex())
}

fn insert_registered_menu(rows: &mut Vec<Row>, parts: &[&str], command: &str) {
    let Some((head, tail)) = parts.split_first() else {
        rows.push(Row::Command {
            id: command.into(),
            target: Target::None,
            label: None,
            path: String::new(),
            checked: None,
        });
        return;
    };
    if tail.is_empty() {
        rows.push(Row::Command {
            id: command.into(),
            target: Target::None,
            label: Some((*head).into()),
            path: String::new(),
            checked: None,
        });
        return;
    }
    let index = rows
        .iter()
        .position(|row| matches!(row, Row::Sub(title, _) if title == head))
        .unwrap_or_else(|| {
            rows.push(Row::Sub((*head).into(), vec![]));
            rows.len() - 1
        });
    if let Row::Sub(_, children) = &mut rows[index] {
        insert_registered_menu(children, tail, command);
    }
}
impl Shell {
    fn row_height(editor: &Editor) -> f32 {
        if editor.state.comfortable() { 30. } else { 26. }
    }
    /// Open a menu bar menu (or switch to it), below its title.
    pub(crate) fn open_bar(&mut self, index: usize, keyboard: bool) {
        let ui = self
            .current
            .as_ref()
            .and_then(|id| self.floats.iter().find(|s| &s.id == id))
            .map_or(&self.ui, |s| &s.ui);
        let title = ui
            .nodes
            .iter()
            .filter_map(|node| {
                match self
                    .bindings
                    .get(&ara_ui::source::stable_id(&node.element.id))
                {
                    Some(Binding::MenuBar(title)) => Some(title.clone()),
                    _ => None,
                }
            })
            .nth(index);
        if let Some(title) = title {
            self.open_bar_title(&title, keyboard);
        }
    }
    /// Open a semantic top-level identity; the next render validates its registration.
    pub(crate) fn open_bar_title(&mut self, title: &str, keyboard: bool) {
        self.menu = Some(MenuState {
            source: Source::Bar(title.into()),
            anchor: None,
            at: [0., 0.],
            surface: None,
            open: Vec::new(),
            active: keyboard.then_some(0),
            query: String::new(),
            painted_rows: Vec::new(),
            context_identity: String::new(),
        });
    }
    /// Open a context or value menu at a point of the current surface.
    pub(crate) fn open_menu_at(&mut self, source: Source, at: [f32; 2]) {
        self.menu = Some(MenuState {
            source,
            anchor: None,
            at,
            surface: self.current.clone(),
            open: Vec::new(),
            active: None,
            query: String::new(),
            painted_rows: Vec::new(),
            context_identity: String::new(),
        });
        self.mnemonics = false;
    }
    /// Open a dropdown below the control that was clicked (`node`, a stable ID), or at
    /// the pointer when it is gone. Clicking the control of the open dropdown closes it.
    pub(crate) fn open_menu_from(&mut self, source: Source, node: u64) {
        let anchor = self
            .ui
            .by_id(node)
            .map(|i| self.ui.nodes[i].element.id.clone());
        if anchor.is_some()
            && self
                .menu
                .as_ref()
                .is_some_and(|m| m.source == source && m.anchor == anchor)
        {
            self.menu = None;
            return;
        }
        let at = [self.pointer[0] + 2., self.pointer[1] + 2.];
        self.open_menu_at(source, at);
        if let Some(menu) = &mut self.menu {
            menu.anchor = anchor;
        }
    }
    /// Open snap increment choices under the pointer.
    pub(crate) fn open_value_menu(&mut self, kind: &'static str) {
        let at = [self.pointer[0] - 8., self.pointer[1] + 12.];
        self.open_menu_at(Source::Values(kind), at);
    }
    fn expand(&self, editor: &Editor, items: &'static [Item], out: &mut Vec<Row>) {
        for item in items {
            match item {
                Item::Command(id) => out.push(Row::Command {
                    id: (*id).into(),
                    target: self.menu_target(),
                    label: None,
                    path: String::new(),
                    checked: None,
                }),
                Item::Separator => {
                    if !matches!(out.last(), None | Some(Row::Sep)) {
                        out.push(Row::Sep);
                    }
                }
                Item::Sub(title, children) => {
                    let mut rows = Vec::new();
                    self.expand(editor, children, &mut rows);
                    out.push(Row::Sub((*title).into(), rows));
                }
                Item::Dynamic(kind) => self.dynamic(editor, *kind, out),
            }
        }
        if matches!(out.last(), Some(Row::Sep)) {
            out.pop();
        }
    }
    fn menu_target(&self) -> Target {
        match &self.menu {
            Some(MenuState {
                source: Source::Context(_, target),
                ..
            }) => target.clone(),
            _ => Target::None,
        }
    }
    fn dynamic(&self, editor: &Editor, kind: Dynamic, out: &mut Vec<Row>) {
        match kind {
            Dynamic::BuildProfiles => {
                let selected = self.selected_build_profile(editor).ok();
                for name in self.build_profiles() {
                    out.push(Row::Command {
                        id: kind.command().into(),
                        label: Some(name.clone()),
                        checked: Some(selected.as_deref() == Some(name.as_str())),
                        target: Target::Text(name),
                        path: String::new(),
                    });
                }
            }
            Dynamic::PrefabSources => {
                let target = self.menu_target();
                let Some(entity) = target.entity(&editor.state) else {
                    return;
                };
                let path = if let Target::Field(path) = &target {
                    Some(path.as_str())
                } else {
                    None
                };
                for source in super::prefab_ops::source_choices(
                    editor.host.store(),
                    &editor.state.doc,
                    entity,
                ) {
                    let logical = editor.host.store().workspace().reference_path(&source.path);
                    out.push(Row::Command {id:kind.command().into(),target:Target::Text(json!({"entity":entity,"path":path,"destination":format!("@{logical}#{}",source.doc)}).to_string()),label:Some(logical.to_owned()),path:String::new(),checked:None});
                }
            }
            Dynamic::RecentScenes => {
                let workspace = editor.host.store().workspace();
                let mut scenes: Vec<_> = workspace
                    .docs()
                    .filter(|(_, d)| d.kind == "scene")
                    .map(|(_, d)| d.id.to_string())
                    .collect();
                // Recently opened first, then the rest by path.
                scenes.sort_by_key(|id| {
                    (
                        editor
                            .state
                            .recent_scenes
                            .iter()
                            .position(|r| r == id)
                            .unwrap_or(usize::MAX),
                        workspace.path_of(id).unwrap_or("").to_owned(),
                    )
                });
                for id in scenes.into_iter().take(12) {
                    let path = workspace.path_of(&id).unwrap_or(&id).to_owned();
                    out.push(Row::Command {
                        id: kind.command().into(),
                        checked: Some(id == editor.state.doc),
                        target: Target::Doc(id),
                        label: Some(path),
                        path: String::new(),
                    });
                }
            }
            Dynamic::SavedLayouts => {
                let names = crate::state::saved_layouts(editor.host.root());
                if !names.is_empty() {
                    out.push(Row::Sep);
                }
                for name in names {
                    out.push(Row::Command {
                        id: kind.command().into(),
                        label: Some(name.clone()),
                        target: Target::Text(name),
                        path: String::new(),
                        checked: None,
                    });
                }
            }
            Dynamic::Windows => {
                if !editor.state.floating.is_empty() && !matches!(out.last(), Some(Row::Sep) | None)
                {
                    out.push(Row::Sep);
                }
                for window in &editor.state.floating {
                    let mut panels = Vec::new();
                    fn collect(n: &ara_ui::docking::DockNode, out: &mut Vec<String>) {
                        match n {
                            ara_ui::docking::DockNode::Area { tabs, .. } => {
                                out.extend(tabs.iter().map(|t| panels::title(t).to_owned()))
                            }
                            ara_ui::docking::DockNode::Split { first, second, .. } => {
                                collect(first, out);
                                collect(second, out);
                            }
                        }
                    }
                    collect(&window.root, &mut panels);
                    out.push(Row::Command {
                        id: kind.command().into(),
                        target: Target::Window(window.id.clone()),
                        label: Some(format!("{} (floating)", panels.join(", "))),
                        path: String::new(),
                        checked: None,
                    });
                }
            }
        }
    }
    /// Rows at each open level; level 0 is filtered by the query.
    pub(crate) fn menu_levels(&self, editor: &Editor) -> Vec<Vec<Row>> {
        let Some(menu) = &self.menu else {
            return Vec::new();
        };
        let mut root = Vec::new();
        match &menu.source {
            Source::Bar(title) => {
                if let Some(menu) = MENUS.iter().find(|menu| menu.title == title.as_str()) {
                    self.expand(editor, menu.items, &mut root);
                }
                let mut placements: Vec<_> = editor.extensions.menus.iter().collect();
                placements.sort_by(|a, b| (&a.path, &a.command).cmp(&(&b.path, &b.command)));
                placements.dedup_by(|a, b| a.path == b.path && a.command == b.command);
                for placement in placements {
                    let parts: Vec<_> = placement.path.split('/').collect();
                    if parts.first().copied() == Some(title.as_str()) {
                        insert_registered_menu(&mut root, &parts[1..], &placement.command);
                    }
                }
            }
            Source::Context(kind, _) => self.expand(editor, menus::context(*kind), &mut root),
            Source::Assets(..) => {}
            Source::Values("fov") => {
                let current = editor.state.camera.fov_y_degrees;
                for v in [30., 35., 40., 46., 50., 54., 60., 70., 90.] {
                    root.push(Row::Command {
                        id: "view.camera.fov.set".into(),
                        target: Target::Text(format!("{v}")),
                        label: Some(format!("{v}°")),
                        path: String::new(),
                        checked: Some((current - v).abs() < 0.05),
                    });
                }
            }
            Source::Values(kind) => {
                let (values, unit, current): (&[f32], &str, f32) = match *kind {
                    "move" => (
                        &[0.01, 0.05, 0.1, 0.25, 0.5, 1., 2., 5., 10.],
                        "m",
                        editor.state.move_snap,
                    ),
                    "rotate" => (
                        &[1., 5., 10., 15., 30., 45., 90.],
                        "°",
                        editor.state.rotate_snap,
                    ),
                    _ => (
                        &[0.01, 0.05, 0.1, 0.25, 0.5, 1.],
                        "×",
                        editor.state.scale_snap,
                    ),
                };
                for v in values {
                    root.push(Row::Command {
                        id: "snap.set".into(),
                        target: Target::Text(format!("{kind}:{v}")),
                        label: Some(format!("{v} {unit}")),
                        path: String::new(),
                        checked: Some((current - v).abs() < 0.0001),
                    });
                }
            }
        }
        if let Source::Assets(path, kind) = &menu.source {
            let current = self.field_value(path);
            // Adding an item by picking its asset offers no "None".
            if !self.appends(path) {
                root.push(Row::Command {
                    id: "field.set".into(),
                    target: Target::Text(format!("{path}=null")),
                    label: Some("None".into()),
                    path: String::new(),
                    checked: Some(current.is_null()),
                });
            }
            let workspace = editor.host.store().workspace();
            for asset in self
                .browser
                .items
                .iter()
                .filter(|a| a.fits(kind.as_deref(), path))
            {
                // Files still importing have no asset ID to refer to yet.
                let Some(reference) = asset.current_reference(workspace) else {
                    continue;
                };
                root.push(Row::Command {
                    id: "field.set".into(),
                    target: Target::Text(format!("{path}={}", json!(reference))),
                    // Without its extension, like Assets (the folder is beside it).
                    label: Some(names::stem(asset.name()).to_owned()),
                    path: asset
                        .path
                        .rsplit_once('/')
                        .map_or(String::new(), |(folder, _)| folder.to_owned()),
                    checked: Some(current.as_str() == Some(reference.as_str())),
                });
            }
        }
        self.dedup_menu_rows(editor, &mut root);
        if !menu.query.is_empty() {
            let query = menu.query.to_lowercase();
            let mut flat = Vec::new();
            fn flatten(rows: Vec<Row>, path: &str, out: &mut Vec<Row>) {
                for row in rows {
                    match row {
                        Row::Command {
                            id,
                            target,
                            label,
                            checked,
                            ..
                        } => out.push(Row::Command {
                            id,
                            target,
                            label,
                            path: path.to_owned(),
                            checked,
                        }),
                        Row::Sub(title, items) => {
                            let path = if path.is_empty() {
                                title.to_owned()
                            } else {
                                format!("{path} › {title}")
                            };
                            flatten(items, &path, out);
                        }
                        Row::Sep => {}
                    }
                }
            }
            flatten(root, "", &mut flat);
            root = flat
                .into_iter()
                .filter(|row| match row {
                    Row::Command { id, label, .. } => {
                        let title = label.clone().unwrap_or_else(|| {
                            commands::lookup(&editor.extensions, id)
                                .map(|c| c.title.to_owned())
                                .unwrap_or_default()
                        });
                        palette::fuzzy(&title.to_lowercase(), &query).is_some()
                    }
                    _ => false,
                })
                .collect();
        }
        let mut levels = vec![root];
        for open in &menu.open {
            let Some(Row::Sub(_, items)) = levels
                .last()
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| matches!(row,Row::Sub(title,_) if title==open))
                })
                .cloned()
            else {
                break;
            };
            levels.push(items);
        }
        levels
    }
    fn row_identity(&self, editor: &Editor, row: &Row) -> String {
        let value = match row {
            Row::Command {
                id,
                target,
                label,
                path,
                ..
            } => {
                let title = label.clone().unwrap_or_else(|| {
                    commands::lookup(&editor.extensions, id)
                        .map(|entry| entry.title(&self.ctx(editor, target)))
                        .unwrap_or_default()
                });
                json!(["command", id, target, title, path])
            }
            Row::Sub(title, _) => json!(["submenu", title]),
            Row::Sep => json!(["separator"]),
        };
        ara_json::to_canonical_string(&value)
    }
    fn dedup_menu_rows(&self, editor: &Editor, rows: &mut Vec<Row>) {
        let mut seen = std::collections::BTreeSet::new();
        for row in rows.iter_mut() {
            if let Row::Sub(_, children) = row {
                self.dedup_menu_rows(editor, children);
            }
        }
        rows.retain(|row| {
            !matches!(row, Row::Command { .. }) || seen.insert(self.row_identity(editor, row))
        });
    }
    fn row_navigation_identity(row: &Row) -> String {
        let value = match row {
            Row::Command {
                id,
                target,
                label,
                path,
                ..
            } => json!(["command", id, target, label, path]),
            Row::Sub(title, _) => json!(["submenu", title]),
            Row::Sep => json!(["separator"]),
        };
        ara_json::to_canonical_string(&value)
    }
    /// Semantic row identity used by rendering and verification; never a sibling index.
    pub(crate) fn menu_row_id(&self, editor: &Editor, level: usize, row: &Row) -> String {
        let Some(menu) = &self.menu else {
            return String::new();
        };
        let value = json!([
            format!("{:?}", menu.source),
            menu.open.get(..level).unwrap_or(&[]),
            self.row_identity(editor, row),
            menu.context_identity,
            editor.extensions.generation
        ]);
        let canonical = ara_json::to_canonical_string(&value);
        format!("m{level}-{}", blake3::hash(canonical.as_bytes()).to_hex())
    }
    /// Reconcile navigation by title/command while invalidating captures on changed code or targets.
    fn sync_menu_navigation(&mut self, editor: &Editor) -> Vec<Vec<Row>> {
        let Some(menu) = self.menu.as_ref() else {
            return Vec::new();
        };
        if let Some(title) = menu.bar()
            && !commands::bar_titles(&editor.extensions).contains(&title)
        {
            self.menu = None;
            return Vec::new();
        }
        let levels = self.menu_levels(editor);
        let rows = levels.last().map(Vec::as_slice).unwrap_or(&[]);
        let identities: Vec<_> = rows.iter().map(Self::row_navigation_identity).collect();
        let context = ara_json::to_canonical_string(&json!([
            editor.state.doc,
            editor.state.selection,
            editor.state.focus,
            editor.state.asset_path,
            editor.state.asset_folder,
            editor.state.resource,
            editor.state.resource_item,
            editor.state.canvas_selection,
            self.asset_selection.selected()
        ]));
        if let Some(menu) = &mut self.menu {
            if !menu.painted_rows.is_empty() {
                menu.active = menu
                    .active
                    .and_then(|active| menu.painted_rows.get(active))
                    .and_then(|prior| identities.iter().position(|identity| identity == prior));
            } else {
                menu.active = menu.active.filter(|i| *i < rows.len());
            }
            menu.open.truncate(levels.len().saturating_sub(1));
            menu.painted_rows = identities;
            menu.context_identity = blake3::hash(context.as_bytes()).to_hex().to_string();
            if let Some(title) = menu.bar() {
                menu.anchor = Some(bar_id(title, editor.extensions.generation.as_deref()));
            }
        }
        levels
    }
    /// Popups for the current surface: menus and context menus.
    pub(super) fn overlays(&mut self, editor: &mut Editor, out: &mut String) -> ara_ui::Result<()> {
        if self
            .menu
            .as_ref()
            .is_none_or(|menu| menu.surface != self.current)
        {
            return Ok(());
        }
        let levels = self.sync_menu_navigation(editor);
        let Some(menu) = self.menu.clone() else {
            return Ok(());
        };
        if menu.surface != self.current {
            return Ok(());
        }
        let keymap = self.keymap.clone();
        for (level, rows) in levels.iter().enumerate() {
            // The root opens below its control (or at the click); each submenu beside
            // the row that opened it. `Ui::frame` flips and shifts them into the window.
            let place = match (level, &menu.anchor) {
                (0, Some(anchor)) => format!(
                    "anchor='{}' placement='bottom-start' offset='2'",
                    escape(anchor)
                ),
                (0, None) => format!(
                    "at='{},{}' placement='bottom-start' offset='0'",
                    menu.at[0], menu.at[1]
                ),
                _ => {
                    let anchor = levels[level - 1]
                        .iter()
                        .find(|row| matches!(row,Row::Sub(title,_) if title==&menu.open[level-1]))
                        .map(|row| self.menu_row_id(editor, level - 1, row))
                        .unwrap_or_default();
                    format!("anchor='{anchor}' placement='right-start' offset='0'")
                }
            };
            out.push_str(&format!(
                "<popover id='menu-pop-{level}' class='menu' {place} style='min-width:{WIDTH}px'>"
            ));
            if level == 0 {
                self.bindings.insert(
                    ara_ui::source::stable_id("menu-filter"),
                    Binding::MenuFilter,
                );
                out.push_str(&format!("<panel id='menu-filter-row' class='menu-filter'><field id='menu-filter' class='search-input' style='height:{}px' text='{}' label='Filter this menu' action='Dispatch' /><image id='menu-filter-icon' class='search-icon' src='icon-search' tint='true' />{}</panel>",Self::row_height(editor)-4.,escape(&menu.query),if menu.query.is_empty(){"<text id='menu-filter-placeholder' class='placeholder' text='Type to filter' />"}else{""}));
                if rows.is_empty() {
                    out.push_str("<text id='menu-empty' class='caption' text='No matching commands' style='padding:6px' />");
                }
            }
            let active = if level + 1 == levels.len() {
                menu.active
            } else {
                menu.open.get(level).and_then(|open| {
                    rows.iter()
                        .position(|row| matches!(row,Row::Sub(title,_) if title==open))
                })
            };
            for (i, row) in rows.iter().enumerate() {
                let id = if matches!(row, Row::Sep) {
                    format!("menu-separator-{level}-{i}")
                } else {
                    self.menu_row_id(editor, level, row)
                };
                match row {
                    Row::Sep => out.push_str(&format!("<panel id='{id}' class='menu-sep'><panel id='{id}-line' class='menu-line' /></panel>")),
                    Row::Sub(title, _) => {
                        self.bindings.insert(ara_ui::source::stable_id(&id), Binding::MenuSub(level, title.clone()));
                        out.push_str(&format!("<button id='{id}' class='menu-item{}' label='{}' action='Dispatch'><panel id='{id}-check' class='menu-check' /><text id='{id}-label' class='menu-label' text='{}' />{}</button>",if active==Some(i){" menu-active"}else{""},escape(title),escape(title),if active==Some(i){icons::icon_in(&id,"chevron-right","on-selection")}else{icons::icon(&id,"chevron-right")}));
                    }
                    Row::Command { id: command, target, label, path, checked } => {
                        let Some(spec) = commands::lookup(&editor.extensions, command) else { continue };
                        let cx = self.ctx(editor, target);
                        let avail = spec.avail(&cx);
                        let title = label.clone().unwrap_or_else(|| spec.title(&cx));
                        let check = checked.or(match avail { Avail::Checked(v) => Some(v), _ => None });
                        let reason = match &avail { Avail::Blocked(r) => r.clone(), _ => spec.summary.to_owned() };
                        self.bindings.insert(ara_ui::source::stable_id(&id), spec.binding(target.clone()));
                        let tint = if active == Some(i) { "on-selection" } else if check == Some(true) { "accent" } else { "" };
                        let icon = if check == Some(true) {
                            icons::icon_in(&format!("{id}-c"), "check", tint)
                        } else if !spec.icon.is_empty() && icons::exists(spec.icon) {
                            icons::icon_in(&format!("{id}-c"), spec.icon, tint)
                        } else {
                            format!("<panel id='{id}-check' class='menu-check' />")
                        };
                        out.push_str(&format!("<button id='{id}' class='menu-item{}' label='{}' description='{}' action='Dispatch'{}>{icon}<text id='{id}-label' class='menu-label' {}text='{}' />",if active==Some(i){" menu-active"}else{""},escape(&title),escape(&reason),if avail.ready(){""}else{" disabled='true'"},if path.is_empty(){""}else{"style='flex-grow:0;flex-basis:auto' "},escape(&title)));
                        if !path.is_empty() {
                            // The name keeps its width; a long folder clips first.
                            out.push_str(&format!("<text id='{id}-path' class='caption' style='white-space:nowrap;overflow:hidden;min-width:0px;flex-shrink:1;flex-basis:0px;flex-grow:1;text-align:right' text='{}' />",escape(path)));
                        }
                        if let Some(chord) = keymap.chords(command).first() {
                            out.push_str(&format!("<panel id='{id}-keys' class='keys'>"));
                            for (n, cap) in chord.caps().iter().enumerate() {
                                out.push_str(&format!("<panel id='{id}-key-{n}' class='keycap'><text id='{id}-key-text-{n}' text='{}' /></panel>",escape(cap)));
                            }
                            out.push_str("</panel>");
                        }
                        out.push_str("</button>");
                    }
                }
            }
            out.push_str("</popover>");
        }
        Ok(())
    }
    /// A press at `point` outside the open popovers closes them (those above the one
    /// it lands in); a press on the open menu's own control is left to toggle it.
    /// Returns whether anything closed.
    pub fn dismiss_popovers(
        &mut self,
        editor: &mut Editor,
        point: [f32; 2],
    ) -> ara_ui::Result<bool> {
        let mut dismissal = self.popovers.pointer_down(&self.ui, point);
        // A pinned colour picker stays open through clicks outside it.
        if self.color_picker.as_ref().is_some_and(|p| p.pinned()) {
            dismissal.close.retain(|id| id != "color-picker");
        }
        if dismissal.close.is_empty() || dismissal.on_anchor {
            return Ok(false);
        }
        self.close_popovers(editor, &dismissal.close)?;
        Ok(true)
    }
    /// Close popovers by ID: a menu level closes with the levels below it (its
    /// submenus); the palette closes through its editor action.
    pub(crate) fn close_popovers(
        &mut self,
        editor: &mut Editor,
        ids: &[String],
    ) -> ara_ui::Result<()> {
        for id in ids {
            if let Some(level) = id
                .strip_prefix("menu-pop-")
                .and_then(|l| l.parse::<usize>().ok())
            {
                match level {
                    0 => {
                        self.menu = None;
                        self.mnemonics = false;
                    }
                    _ => {
                        if let Some(menu) = &mut self.menu {
                            menu.open.truncate(level - 1);
                            menu.active = None;
                        }
                    }
                }
            } else if id == "palette" && editor.state.palette {
                editor.action("editor.palette", json!({"open": false}))?;
            } else if id == "add-component-pop" {
                self.add_component = None;
            } else if id == "color-picker" {
                // A click outside keeps the colour, as Enter does.
                self.close_color_picker(editor, false)?;
            }
        }
        self.rebuild(editor)
    }
    /// Pointer hover: switch menu bar menus and open submenus without a click.
    pub(crate) fn menu_hover(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(menu) = &self.menu else {
            return Ok(false);
        };
        let Some(hit) = self.ui.hit_test(self.pointer[0], self.pointer[1]) else {
            return Ok(false);
        };
        let binding = self
            .bindings
            .get(&ara_ui::source::stable_id(&self.ui.nodes[hit].element.id));
        let change = match binding {
            Some(Binding::MenuBar(title)) if menu.bar().is_some_and(|b| b != title.as_str()) => {
                let title = title.clone();
                self.open_bar_title(&title, false);
                true
            }
            Some(Binding::MenuSub(level, row)) if menu.open.get(*level) != Some(row) => {
                let (level, row) = (*level, row.clone());
                if let Some(m) = &mut self.menu {
                    m.open.truncate(level);
                    m.open.push(row);
                    m.active = None;
                }
                true
            }
            Some(Binding::Command(..) | Binding::CommandOwned(..)) => {
                // Hovering a row in a shallower level closes deeper submenus.
                let level = self.ui.nodes[hit].element.id[1..]
                    .split('-')
                    .next()
                    .and_then(|l| l.parse::<usize>().ok());
                match level {
                    Some(level) if menu.open.len() > level => {
                        if let Some(m) = &mut self.menu {
                            m.open.truncate(level);
                        }
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if change {
            self.rebuild(editor)?;
        }
        Ok(change)
    }
    /// Keyboard navigation inside an open menu; returns whether the key was used.
    pub(crate) fn menu_key(&mut self, editor: &mut Editor, scancode: i32) -> ara_ui::Result<bool> {
        let levels = self.sync_menu_navigation(editor);
        let Some(menu) = self.menu.clone() else {
            return Ok(false);
        };
        let rows = levels.last().cloned().unwrap_or_default();
        let selectable = |i: usize| match &rows[i] {
            Row::Sep => false,
            Row::Sub(..) => true,
            Row::Command { id, target, .. } => commands::lookup(&editor.extensions, id)
                .is_some_and(|c| c.avail(&self.ctx(editor, target)).ready()),
        };
        let step = |from: Option<usize>, down: bool| -> Option<usize> {
            let n = rows.len();
            if n == 0 {
                return None;
            }
            let mut i = from.unwrap_or(if down { n - 1 } else { 0 });
            for _ in 0..n {
                i = if down { (i + 1) % n } else { (i + n - 1) % n };
                if selectable(i) {
                    return Some(i);
                }
            }
            None
        };
        let mut next = menu.clone();
        match scancode {
            81 => next.active = step(menu.active, true),
            82 => next.active = step(menu.active, false),
            74 => next.active = (0..rows.len()).find(|index| selectable(*index)),
            77 => next.active = (0..rows.len()).rfind(|index| selectable(*index)),
            79 => match menu.active.and_then(|i| rows.get(i).map(|r| (i, r))) {
                Some((_, Row::Sub(title, _))) => {
                    next.open.push(title.clone());
                    next.active = None;
                }
                _ => {
                    if let Some(b) = menu.bar() {
                        let titles = commands::bar_titles(&editor.extensions);
                        if let Some(index) = titles.iter().position(|title| *title == b) {
                            self.open_bar_title(titles[(index + 1) % titles.len()], true);
                        }
                        self.rebuild(editor)?;
                        return Ok(true);
                    }
                }
            },
            80 => {
                if next.open.pop().is_none()
                    && let Some(b) = menu.bar()
                {
                    let titles = commands::bar_titles(&editor.extensions);
                    if let Some(index) = titles.iter().position(|title| *title == b) {
                        self.open_bar_title(
                            titles[(index + titles.len() - 1) % titles.len()],
                            true,
                        );
                    }
                    self.rebuild(editor)?;
                    return Ok(true);
                }
                next.active = None;
            }
            40 | 88 => {
                match menu.active.and_then(|i| rows.get(i).cloned()) {
                    Some(Row::Sub(title, _)) => {
                        next.open.push(title);
                        next.active = None;
                    }
                    Some(Row::Command { id, target, .. }) => {
                        self.menu = None;
                        self.mnemonics = false;
                        self.remember(&id);
                        self.run_command(editor, &id, &target)?;
                        return Ok(true);
                    }
                    _ => {
                        // Enter with a filter runs the first match.
                        if let Some(Row::Command { id, target, .. }) = rows
                            .iter()
                            .find(|r| matches!(r, Row::Command { .. }))
                            .cloned()
                            && !menu.query.is_empty()
                        {
                            self.menu = None;
                            self.run_command(editor, &id, &target)?;
                            return Ok(true);
                        }
                    }
                }
            }
            41 => {
                if next.open.pop().is_none() {
                    self.menu = None;
                    self.mnemonics = false;
                    self.rebuild(editor)?;
                    return Ok(true);
                }
            }
            _ => return Ok(false),
        }
        self.menu = Some(next);
        self.rebuild(editor)?;
        // A wrapped row's actual height is known only after the menu is mounted.
        // Keep keyboard highlight visible without taking focus from its filter.
        let levels = self.menu_levels(editor);
        if let Some(active) = self.menu.as_ref().and_then(|menu| menu.active)
            && let Some(level) = levels.len().checked_sub(1)
            && let Some(row) = levels[level].get(active)
        {
            let id = self.menu_row_id(editor, level, row);
            if let Some(index) = self.shown_node(&id) {
                self.ui.scroll_into_view(index)?;
                self.frame()?;
            }
        }
        Ok(true)
    }
    /// Remember a command for the palette's recent list.
    pub(crate) fn remember(&mut self, id: &str) {
        self.recent_commands.retain(|c| *c != id);
        self.recent_commands.insert(0, id.into());
        self.recent_commands.truncate(8);
    }
    /// What was right-clicked (or is focused) at a point of the current surface.
    pub(crate) fn context_at(&self, editor: &Editor, point: [f32; 2]) -> Option<(Context, Target)> {
        for index in self.ui.paint_order.iter().rev() {
            let n = &self.ui.nodes[*index];
            if !n.visible
                || !n.rect.contains(point[0], point[1])
                || !n.clip.contains(point[0], point[1])
            {
                continue;
            }
            let id = n.element.id.as_str();
            if let Some(binding) = self.bindings.get(&ara_ui::source::stable_id(id)) {
                match binding {
                    Binding::Command("history.inspect", target) => {
                        return Some((Context::History, target.clone()));
                    }
                    Binding::AssetFolder(path, _) => {
                        return Some((Context::Folder, Target::Folder(path.clone())));
                    }
                    Binding::ContextMenu(context, target) => {
                        return Some((*context, target.clone()));
                    }
                    Binding::Select(entity) => {
                        return Some((Context::Entity, Target::Entity(entity.clone())));
                    }
                    Binding::Asset(asset) => {
                        return Some((Context::Asset, Target::Asset(asset.path.clone())));
                    }
                    Binding::Field(path, _)
                    | Binding::Scrub(path, _)
                    | Binding::ColorPicker(path, _)
                    | Binding::PickField(path) => {
                        return Some((Context::Field, Target::Field(path.clone())));
                    }
                    Binding::ComponentCollapse(name) => {
                        return Some((Context::Component, Target::Component(name.clone())));
                    }
                    Binding::Diagnostic(d) => {
                        return Some((
                            Context::Console,
                            Target::Text(format!("{} {}", d.code, d.message)),
                        ));
                    }
                    Binding::Protocol("editor.workspace", params)
                        if params.get("item").is_some() =>
                    {
                        return Some((
                            Context::Module,
                            Target::Text(params["item"].as_str().unwrap_or("").into()),
                        ));
                    }
                    Binding::Protocol("editor.workspace", params)
                        if params.get("folder").is_some() =>
                    {
                        return Some((
                            Context::Folder,
                            Target::Folder(params["folder"].as_str().unwrap_or("").into()),
                        ));
                    }
                    _ => {}
                }
            }
            if let Some(panel) = id
                .strip_prefix("tab-")
                .or_else(|| id.strip_prefix("header-"))
            {
                return Some((Context::Tab, Target::Panel(panel.to_owned())));
            }
            if let Some(path) = id
                .strip_prefix("field-")
                .or_else(|| id.strip_prefix("prop-"))
                && let Some(field) = self.fields.keys().find(|f| f.replace('.', "-") == path)
            {
                return Some((Context::Field, Target::Field(field.clone())));
            }
            if let Some(name) = id.strip_prefix("component-") {
                return Some((Context::Component, Target::Component(name.to_owned())));
            }
            if id.starts_with("chip-") || id.starts_with("status-") {
                return Some((Context::Status, Target::None));
            }
            match id {
                "hierarchy-rows" | "hierarchy-empty" => {
                    return Some((Context::Hierarchy, Target::None));
                }
                "asset-grid" | "asset-rows" | "assets-content" | "assets-empty" => {
                    return Some((Context::Assets, Target::None));
                }
                canvas
                    if canvas
                        .strip_suffix("-canvas")
                        .is_some_and(|page| graph_panel::GRAPH_PAGES.contains(&page)) =>
                {
                    let node = canvas
                        .strip_suffix("-canvas")
                        .and_then(|page| self.graphs.get(page))
                        .and_then(|g| g.canvas.node_at(point, n.rect));
                    return Some(match node {
                        Some(node) => (Context::Node, Target::Node(node)),
                        None => (Context::Canvas, Target::None),
                    });
                }
                "profiler-body" => return Some((Context::Profiler, Target::None)),
                "console-scroll" => return Some((Context::Console, Target::None)),
                "history-transactions" | "history-detail" => {
                    return Some((Context::History, Target::None));
                }
                _ => {}
            }
        }
        let _ = editor;
        None
    }
    /// Open a context menu for whatever is at the pointer; false if nothing applies.
    pub(crate) fn open_context(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some((kind, target)) = self.context_at(editor, self.pointer) else {
            return Ok(false);
        };
        self.open_menu_at(
            Source::Context(kind, target),
            [self.pointer[0] + 2., self.pointer[1] + 2.],
        );
        self.rebuild(editor)?;
        Ok(true)
    }
}

impl Shell {
    /// Viewport right-click: an entity menu for a picked object, else the empty-space menu.
    pub(crate) fn open_viewport_menu(
        &mut self,
        editor: &mut Editor,
        context: Context,
        target: Target,
    ) -> ara_ui::Result<()> {
        self.open_menu_at(
            Source::Context(context, target),
            [self.pointer[0] + 2., self.pointer[1] + 2.],
        );
        self.rebuild(editor)
    }
}

#[cfg(test)]
#[path = "menu/capture_tests.rs"]
mod capture_tests;
#[cfg(test)]
#[path = "menu/prefab_source_tests.rs"]
mod prefab_source_tests;
#[cfg(test)]
#[path = "menu/scroll_tests.rs"]
mod scroll_tests;
