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
    /// A top-level menu by index.
    Bar(usize),
    /// A right-click (or ⋯) menu for a target.
    Context(Context, Target),
    /// Snap increments: `move`, `rotate` or `scale`.
    Values(&'static str),
    /// An inspector enum field's options.
    Enum(String, Vec<String>),
}
/// An open menu: its root anchor, open submenus and keyboard highlight.
#[derive(Clone, Debug)]
pub(crate) struct MenuState {
    pub source: Source,
    /// Root popup top-left, in the surface's logical pixels.
    pub at: [f32; 2],
    /// Surface that shows the menu; `None` is the main window.
    pub surface: Option<String>,
    /// Open submenu row per level.
    pub open: Vec<usize>,
    /// Keyboard highlight in the deepest level.
    pub active: Option<usize>,
    /// Type-to-filter text (root level).
    pub query: String,
}
impl MenuState {
    pub fn bar(&self) -> Option<usize> {
        match self.source {
            Source::Bar(i) => Some(i),
            _ => None,
        }
    }
}
/// A resolved row.
#[derive(Clone, Debug)]
pub(crate) enum Row {
    Command {
        id: &'static str,
        target: Target,
        /// Title override (dynamic and value rows) or a filter-result path.
        label: Option<String>,
        path: String,
        checked: Option<bool>,
    },
    Sub(&'static str, &'static [Item]),
    Sep,
}
const WIDTH: f32 = 272.;
impl Shell {
    fn row_height(editor: &Editor) -> f32 {
        if editor.state.comfortable() { 30. } else { 26. }
    }
    /// Open a menu bar menu (or switch to it).
    pub(crate) fn open_bar(&mut self, index: usize, keyboard: bool) {
        let x = self.menu_anchors.get(index).copied().unwrap_or(40.);
        self.menu = Some(MenuState {
            source: Source::Bar(index),
            at: [x, 32.],
            surface: None,
            open: Vec::new(),
            active: keyboard.then_some(0),
            query: String::new(),
        });
    }
    /// Open a context or value menu at the pointer of the current surface.
    pub(crate) fn open_menu_at(&mut self, source: Source, at: [f32; 2]) {
        self.menu = Some(MenuState {
            source,
            at,
            surface: self.current.clone(),
            open: Vec::new(),
            active: None,
            query: String::new(),
        });
        self.mnemonics = false;
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
                    id,
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
                Item::Sub(title, children) => out.push(Row::Sub(title, children)),
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
                        self.recent_docs
                            .iter()
                            .position(|r| r == id)
                            .unwrap_or(usize::MAX),
                        workspace.path_of(id).unwrap_or("").to_owned(),
                    )
                });
                for id in scenes.into_iter().take(12) {
                    let path = workspace.path_of(&id).unwrap_or(&id).to_owned();
                    out.push(Row::Command {
                        id: kind.command(),
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
                        id: kind.command(),
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
                        id: kind.command(),
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
            Source::Bar(i) => self.expand(editor, MENUS[*i].items, &mut root),
            Source::Context(kind, _) => self.expand(editor, menus::context(*kind), &mut root),
            Source::Enum(..) => {}
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
                        id: "snap.set",
                        target: Target::Text(format!("{kind}:{v}")),
                        label: Some(format!("{v} {unit}")),
                        path: String::new(),
                        checked: Some((current - v).abs() < 0.0001),
                    });
                }
            }
        }
        if let Source::Enum(path, options) = &menu.source {
            let current = self
                .fields
                .get(path)
                .map(|f| f.value.clone())
                .unwrap_or(Value::Null);
            for option in options {
                root.push(Row::Command {
                    id: "field.set",
                    target: Target::Text(format!("{path}={}", json!(option))),
                    label: Some(super::inspector_panel::sentence(option)),
                    path: String::new(),
                    checked: Some(current.as_str() == Some(option)),
                });
            }
        }
        if !menu.query.is_empty() {
            let query = menu.query.to_lowercase();
            let mut flat = Vec::new();
            fn flatten(
                shell: &Shell,
                editor: &Editor,
                rows: Vec<Row>,
                path: &str,
                out: &mut Vec<Row>,
            ) {
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
                            let mut children = Vec::new();
                            shell.expand(editor, items, &mut children);
                            let path = if path.is_empty() {
                                title.to_owned()
                            } else {
                                format!("{path} › {title}")
                            };
                            flatten(shell, editor, children, &path, out);
                        }
                        Row::Sep => {}
                    }
                }
            }
            flatten(self, editor, root, "", &mut flat);
            root = flat
                .into_iter()
                .filter(|row| match row {
                    Row::Command { id, label, .. } => {
                        let title = label.clone().unwrap_or_else(|| {
                            commands::get(id)
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
            let Some(Row::Sub(_, items)) = levels.last().and_then(|l| l.get(*open)).cloned() else {
                break;
            };
            let mut rows = Vec::new();
            self.expand(editor, items, &mut rows);
            levels.push(rows);
        }
        levels
    }
    /// Top-left of each open popup level, clamped to the surface.
    fn popup_origins(&self, editor: &Editor, levels: &[Vec<Row>]) -> Vec<[f32; 2]> {
        let Some(menu) = &self.menu else {
            return Vec::new();
        };
        let item = Self::row_height(editor);
        let filter = item + 4.;
        let size = self.ui.size;
        let height = |rows: &[Row], top: bool| {
            8. + if top { filter } else { 0. }
                + rows
                    .iter()
                    .map(|r| if matches!(r, Row::Sep) { 9. } else { item })
                    .sum::<f32>()
        };
        let mut out: Vec<[f32; 2]> = Vec::new();
        for (level, rows) in levels.iter().enumerate() {
            let h = height(rows, level == 0);
            let [mut x, mut y] = if level == 0 {
                menu.at
            } else {
                let parent = out[level - 1];
                let index = menu.open[level - 1];
                let before: f32 = levels[level - 1][..index]
                    .iter()
                    .map(|r| if matches!(r, Row::Sep) { 9. } else { item })
                    .sum();
                let mut x = parent[0] + WIDTH - 4.;
                if x + WIDTH > size[0] {
                    x = parent[0] - WIDTH + 4.;
                }
                [
                    x,
                    parent[1] + 4. + if level == 1 { filter } else { 0. } + before - 4.,
                ]
            };
            x = x.clamp(0., (size[0] - WIDTH).max(0.));
            y = y.min((size[1] - h).max(0.)).max(0.);
            out.push([x, y]);
        }
        out
    }
    /// Popups for the current surface: menus and context menus.
    pub(super) fn overlays(&mut self, editor: &mut Editor, out: &mut String) -> ara_ui::Result<()> {
        let Some(menu) = self.menu.clone() else {
            return Ok(());
        };
        if menu.surface != self.current {
            return Ok(());
        }
        let levels = self.menu_levels(editor);
        let origins = self.popup_origins(editor, &levels);
        let keymap = self.keymap.clone();
        for (level, rows) in levels.iter().enumerate() {
            let [x, y] = origins[level];
            out.push_str(&format!("<panel id='menu-pop-{level}' class='menu' style='left:{x}px;top:{y}px;width:{WIDTH}px'>"));
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
                menu.open.get(level).copied()
            };
            for (i, row) in rows.iter().enumerate() {
                let id = format!("m{level}-{i}");
                match row {
                    Row::Sep => out.push_str(&format!("<panel id='{id}' class='menu-sep'><panel id='{id}-line' class='menu-line' /></panel>")),
                    Row::Sub(title, _) => {
                        self.bindings.insert(ara_ui::source::stable_id(&id), Binding::MenuSub(level, i));
                        out.push_str(&format!("<button id='{id}' class='menu-item{}' label='{}' action='Dispatch'><panel id='{id}-check' class='menu-check' /><text id='{id}-label' class='menu-label' text='{}' />{}</button>",if active==Some(i){" menu-active"}else{""},escape(title),escape(title),if active==Some(i){icons::icon_in(&id,"chevron-right","on-selection")}else{icons::icon(&id,"chevron-right")}));
                    }
                    Row::Command { id: command, target, label, path, checked } => {
                        let Some(spec) = commands::get(command) else { continue };
                        let cx = self.ctx(editor, target);
                        let avail = spec.avail(&cx);
                        let title = label.clone().unwrap_or_else(|| spec.title(&cx));
                        let check = checked.or(match avail { Avail::Checked(v) => Some(v), _ => None });
                        let reason = match &avail { Avail::Blocked(r) => r.clone(), _ => spec.summary.to_owned() };
                        self.bindings.insert(ara_ui::source::stable_id(&id), Binding::Command(command, target.clone()));
                        let tint = if active == Some(i) { "on-selection" } else if check == Some(true) { "accent" } else { "" };
                        let icon = if check == Some(true) {
                            icons::icon_in(&format!("{id}-c"), "check", tint)
                        } else if !spec.icon.is_empty() && icons::exists(spec.icon) {
                            icons::icon_in(&format!("{id}-c"), spec.icon, tint)
                        } else {
                            format!("<panel id='{id}-check' class='menu-check' />")
                        };
                        out.push_str(&format!("<button id='{id}' class='menu-item{}' label='{}' description='{}' action='Dispatch'{}>{icon}<text id='{id}-label' class='menu-label' text='{}' />",if active==Some(i){" menu-active"}else{""},escape(&title),escape(&reason),if avail.ready(){""}else{" disabled='true'"},escape(&title)));
                        if !path.is_empty() {
                            out.push_str(&format!("<text id='{id}-path' class='caption' text='{}' />",escape(path)));
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
            out.push_str("</panel>");
        }
        Ok(())
    }
    /// Whether a point is inside an open popup of the current surface.
    pub(crate) fn menu_contains(&self, point: [f32; 2]) -> bool {
        self.ui
            .nodes
            .iter()
            .any(|n| n.element.id.starts_with("menu-pop-") && n.rect.contains(point[0], point[1]))
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
            Some(Binding::MenuBar(i)) if menu.bar().is_some_and(|b| b != *i) => {
                let i = *i;
                self.open_bar(i, false);
                true
            }
            Some(Binding::MenuSub(level, row)) if menu.open.get(*level) != Some(row) => {
                let (level, row) = (*level, *row);
                if let Some(m) = &mut self.menu {
                    m.open.truncate(level);
                    m.open.push(row);
                    m.active = None;
                }
                true
            }
            Some(Binding::Command(..)) => {
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
        let Some(menu) = self.menu.clone() else {
            return Ok(false);
        };
        let levels = self.menu_levels(editor);
        let rows = levels.last().cloned().unwrap_or_default();
        let selectable = |i: usize| match &rows[i] {
            Row::Sep => false,
            Row::Sub(..) => true,
            Row::Command { id, target, .. } => {
                commands::get(id).is_some_and(|c| c.avail(&self.ctx(editor, target)).ready())
            }
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
            79 => match menu.active.and_then(|i| rows.get(i).map(|r| (i, r))) {
                Some((i, Row::Sub(..))) => {
                    next.open.push(i);
                    next.active = None;
                }
                _ => {
                    if let Some(b) = menu.bar() {
                        self.open_bar((b + 1) % MENUS.len(), true);
                        self.rebuild(editor)?;
                        return Ok(true);
                    }
                }
            },
            80 => {
                if next.open.pop().is_none()
                    && let Some(b) = menu.bar()
                {
                    self.open_bar((b + MENUS.len() - 1) % MENUS.len(), true);
                    self.rebuild(editor)?;
                    return Ok(true);
                }
                next.active = None;
            }
            40 | 88 => {
                match menu.active.and_then(|i| rows.get(i).cloned()) {
                    Some(Row::Sub(..)) => {
                        next.open.push(menu.active.unwrap_or(0));
                        next.active = None;
                    }
                    Some(Row::Command { id, target, .. }) => {
                        self.menu = None;
                        self.mnemonics = false;
                        self.remember(id);
                        self.run_command(editor, id, &target)?;
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
                            self.run_command(editor, id, &target)?;
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
        Ok(true)
    }
    /// Remember a command for the palette's recent list.
    pub(crate) fn remember(&mut self, id: &'static str) {
        self.recent_commands.retain(|c| *c != id);
        self.recent_commands.insert(0, id);
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
                    Binding::Select(entity) => {
                        return Some((Context::Entity, Target::Entity(entity.clone())));
                    }
                    Binding::Asset(asset) => {
                        return Some((Context::Asset, Target::Asset(asset.path.clone())));
                    }
                    Binding::Field(path, _)
                    | Binding::Scrub(path, _)
                    | Binding::ColorPicker(path)
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
                "graph-canvas" => {
                    let node = self.canvas.as_ref().and_then(|c| c.node_at(point, n.rect));
                    return Some(match node {
                        Some(node) => (Context::Node, Target::Node(node)),
                        None => (Context::Canvas, Target::None),
                    });
                }
                "profiler-body" => return Some((Context::Profiler, Target::None)),
                "console-scroll" => return Some((Context::Console, Target::None)),
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
