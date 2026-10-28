//! The command registry: one table drives menus, the palette, context menus,
//! shortcuts, toolbar buttons and the MCP `editor.commands` export.
//!
//! Each [`Spec`] has a stable id, a human title, a default (Ara keymap) shortcut,
//! an availability check that explains why it is disabled, and how it runs.
//! Menu paths are derived from [`menus::MENUS`], so they never drift.
mod extensions;
pub(crate) mod keys;
pub(crate) mod menus;
mod table;
use super::Shell;
use crate::{Editor, state::EditorState};
pub(crate) use extensions::{bar_titles, entries, lookup, registered_menu_path};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What a command acts on: the right-clicked row, field, tab or nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    /// The current selection or focused panel.
    #[default]
    None,
    /// A scene entity address.
    Entity(String),
    /// A project-relative asset path.
    Asset(String),
    /// A project-relative folder.
    Folder(String),
    /// An inspector field path (`Transform.position`).
    Field(String),
    /// An inspector component name.
    Component(String),
    /// A dock panel id.
    Panel(String),
    /// A graph node id in the active resource.
    Node(String),
    /// A scene document id (Open Recent).
    Doc(String),
    /// A floating window id.
    Window(String),
    /// Plain text (console rows, profiler spans).
    Text(String),
}
impl Target {
    /// The entity this command applies to: the target, else the primary selection.
    pub fn entity<'a>(&'a self, state: &'a EditorState) -> Option<&'a str> {
        match self {
            Self::Entity(e) => Some(e),
            _ => state.selection.first().map(String::as_str),
        }
    }
    /// Entities this command applies to: the target if it is outside the selection.
    pub fn entities(&self, state: &EditorState) -> Vec<String> {
        match self {
            Self::Entity(e) if !state.selection.contains(e) => vec![e.clone()],
            _ => state.selection.clone(),
        }
    }
}
/// Inputs to availability checks; built from navigation state and the op store.
pub(crate) struct Ctx<'a> {
    pub state: &'a EditorState,
    pub extensions: &'a crate::extensions::Catalog,
    pub store: &'a ara_ops::Store,
    pub target: &'a Target,
    /// The shell holds copied entities (Paste is available).
    pub clipboard_entities: bool,
    /// The shell holds a copied field value.
    pub clipboard_value: bool,
    /// The clipboard holds values of the targeted component type.
    pub clipboard_component: bool,
    /// Targeted component's current presentation index and section count.
    pub component_position: Option<(usize, usize)>,
    /// Console and Problems filters (for their toggle states).
    pub logs: crate::shell::LogView,
    /// Assets lists every file (Show All Files).
    pub assets_show_all: bool,
    /// Game view: stats overlay on, aspect preset, a frame is showing.
    pub game: (bool, &'static str, bool),
    /// Play was started and has no live world yet: Stop cancels the start.
    pub starting: bool,
    /// The Scene view is explicitly piloting one authored Camera.
    pub pilot: bool,
    /// Cached renderer/UI reason why the targeted probe cannot be captured.
    pub probe_capture_blocker: Option<&'a str>,
    /// Cached reason why no owned capture request can be cancelled.
    pub probe_cancel_blocker: Option<&'a str>,
    /// Build menu state.
    pub build: BuildState,
}
/// What the Build menu's availability depends on.
pub struct BuildState {
    /// A build or a headless Run Build is in progress.
    pub running: bool,
    /// A finished build exists (Run Build, Open Build Folder).
    pub built: bool,
    /// The project has an export profile (`settings/builds/`, or legacy `presets/`).
    pub profile: bool,
}
/// Whether a command can run now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Avail {
    /// Runnable.
    Ready,
    /// Runnable toggle, showing its current state as a check mark.
    Checked(bool),
    /// Not runnable; the reason is shown in menus and tooltips.
    Blocked(String),
}
impl Avail {
    pub fn ready(&self) -> bool {
        !matches!(self, Self::Blocked(_))
    }
}
pub(crate) type RunFn = fn(&mut Shell, &mut Editor, &Target) -> ara_ui::Result<()>;
/// How a command runs.
#[derive(Clone, Copy)]
pub(crate) enum Run {
    /// Runs on the window thread at a frame boundary.
    Do(RunFn),
    /// Registered but not built yet; the reason explains what is missing.
    Later(&'static str),
}
/// One registry command.
#[derive(Clone, Copy)]
pub(crate) struct Spec {
    pub id: &'static str,
    pub title: &'static str,
    /// Lucide icon name, or empty.
    pub icon: &'static str,
    /// Default shortcut list (`Ctrl+Y / Ctrl+Shift+Z`), or empty.
    pub keys: &'static str,
    /// One sentence for the palette, tooltips and MCP.
    pub summary: &'static str,
    pub check: fn(&Ctx) -> Avail,
    /// Dynamic title, e.g. `Undo "Move entity"`.
    pub label: Option<fn(&Ctx) -> String>,
    pub run: Run,
}
impl Spec {
    /// Availability, including commands that are not built yet.
    pub fn avail(&self, cx: &Ctx) -> Avail {
        match self.run {
            Run::Later(reason) => Avail::Blocked(reason.into()),
            Run::Do(_) => (self.check)(cx),
        }
    }
    pub fn title(&self, cx: &Ctx) -> String {
        self.label.map_or_else(|| self.title.to_owned(), |f| f(cx))
    }
}
/// Every registered command, in table order.
pub(crate) fn all() -> &'static [Spec] {
    table::COMMANDS.as_slice()
}
/// Look up a command by id.
pub(crate) fn get(id: &str) -> Option<&'static Spec> {
    table::COMMANDS.iter().find(|c| c.id == id)
}
/// The menu path of a command, e.g. `Edit › Snapping`, or empty.
pub(crate) fn menu_path(id: &str) -> String {
    fn walk(items: &[menus::Item], id: &str, path: &mut Vec<&'static str>) -> bool {
        for item in items {
            match item {
                menus::Item::Command(c) if *c == id => return true,
                menus::Item::Sub(title, children) => {
                    path.push(title);
                    if walk(children, id, path) {
                        return true;
                    }
                    path.pop();
                }
                _ => {}
            }
        }
        false
    }
    for menu in menus::MENUS {
        let mut path = vec![menu.title];
        if walk(menu.items, id, &mut path) {
            return path.join(" › ");
        }
    }
    String::new()
}
/// The registry as agents see it: ids, titles, menu paths, bindings and availability.
pub(crate) fn export(cx: &Ctx, keymap: &keys::Keymap) -> Value {
    json!(
        entries(cx.extensions)
            .map(|c| {
                let avail = c.avail(cx);
                json!({
                    "id": c.id,
                    "title": c.title(cx),
                    "menu": registered_menu_path(cx.extensions, c.id),
                    "summary": c.summary,
                    "shortcuts": keymap.chords(c.id).iter().map(|k| k.text()).collect::<Vec<_>>(),
                    "enabled": avail.ready(),
                    "checked": match avail { Avail::Checked(v) => Some(v), _ => None },
                    "disabledReason": match &avail { Avail::Blocked(r) => Some(r.clone()), _ => None },
                })
            })
            .collect::<Vec<_>>()
    )
}
/// The keymap for the current preferences.
pub(crate) fn keymap(state: &EditorState, extensions: &crate::extensions::Catalog) -> keys::Keymap {
    let preset = keys::Preset::parse(
        state
            .settings
            .get("input.keymap")
            .and_then(Value::as_str)
            .unwrap_or("ara"),
    );
    let overrides = state
        .settings
        .get("input.shortcuts")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    keys::Keymap::new(
        entries(extensions).map(|c| (c.id, c.keys)),
        preset,
        &overrides,
    )
}
impl Shell {
    /// Availability inputs for the current editor state and a target.
    pub(crate) fn ctx<'a>(&'a self, editor: &'a Editor, target: &'a Target) -> Ctx<'a> {
        Ctx {
            state: &editor.state,
            extensions: &editor.extensions,
            store: editor.host.store(),
            target,
            clipboard_entities: self.entity_clipboard.is_some(),
            clipboard_value: self.field_clipboard.is_some(),
            clipboard_component: matches!((target, &self.component_clipboard), (Target::Component(target), Some((name, _))) if target == name),
            component_position: match target {
                Target::Component(name) => self.inspector_sections.position(name),
                _ => None,
            },
            logs: self.log_view,
            assets_show_all: self.assets_show_all,
            game: (self.game_stats, self.game_aspect, self.game_live),
            pilot: self.pilot_active(),
            probe_capture_blocker: self.probe_capture_blocker(editor, target),
            probe_cancel_blocker: self.probe_cancel_blocker(editor, target),
            starting: editor.state.playing.is_none()
                && (self.game_start.is_some() || editor.play_world.is_some()),
            build: BuildState {
                running: self.build.is_some() || self.run.is_some(),
                built: self.last_build.is_some(),
                profile: self.browser.items.iter().any(|a| {
                    (a.path.starts_with("settings/builds/") || a.path.starts_with("presets/"))
                        && a.path.ends_with(".export.json")
                }),
            },
        }
    }
    /// Run a registry command if it is available, then rebuild; disabled commands
    /// report their reason.
    pub fn run_command(
        &mut self,
        editor: &mut Editor,
        id: &str,
        target: &Target,
    ) -> ara_ui::Result<()> {
        self.execute(editor, id, target)?;
        self.rebuild(editor)
    }
    /// Run a registry command without rebuilding (the caller rebuilds once).
    pub(crate) fn execute(
        &mut self,
        editor: &mut Editor,
        id: &str,
        target: &Target,
    ) -> ara_ui::Result<()> {
        if self.offscreen
            && matches!(
                id,
                "assets.reveal"
                    | "assets.open_ide"
                    | "build.open_folder"
                    | "tools.terminal"
                    | "help.docs"
                    | "help.logs"
            )
        {
            return Err(
                "Opening desktop applications is unavailable during offscreen verification".into(),
            );
        }
        if let Some(spec) = lookup(&editor.extensions, id)
            && spec.builtin.is_none()
        {
            if let Avail::Blocked(reason) = spec.avail(&self.ctx(editor, target)) {
                return Err(format!("{}: {reason}", spec.title).into());
            }
            self.menu = None;
            editor.action("editor.command", json!({"id":id,"target":target}))?;
            return Ok(());
        }
        let spec = get(id).ok_or_else(|| format!("unknown command `{id}`"))?;
        let avail = {
            let cx = self.ctx(editor, target);
            spec.avail(&cx)
        };
        if let Avail::Blocked(reason) = avail {
            return Err(format!("{}: {reason}", spec.title).into());
        }
        let Run::Do(run) = spec.run else {
            return Err(format!("{} is not available", spec.title).into());
        };
        if self.confirm_command(editor, id, target)? {
            return Ok(());
        }
        self.menu = None;
        run(self, editor, target)
    }
    /// Run commands queued by `editor.command` (MCP) at this frame boundary.
    pub fn run_queued(&mut self, editor: &mut Editor) -> bool {
        let queued = std::mem::take(&mut editor.queued_commands);
        let ran = !queued.is_empty();
        let bypass = self.confirmation_bypass;
        self.confirmation_bypass = true;
        for (id, target) in queued {
            if let Err(error) = self.run_command(editor, &id, &target) {
                editor.report_failure(&*error);
            }
        }
        self.confirmation_bypass = bypass;
        ran
    }
}
