//! Reflected editor actions. Each successful navigation command produces reversible ops.
use crate::state::{EditorState, Navigation, Tool, default_layout};
use ara_diag::Diagnostic;
use ara_host::{Cx, Method, Service, parse};
use ara_schema::{Schema, SchemaDb};
use ara_ui::docking::{Axis, DockDrop, DockLayout, DockNode};
use glam::Vec3;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::mpsc::Sender;

mod params;
use params::*;

pub(crate) struct EditorService {
    pub navigation: Navigation,
    pub settings: crate::settings::Settings,
    pub changes: Sender<EditorState>,
    pub controls: Sender<Control>,
}
pub(crate) enum Control {
    ProfileFreeze {
        baseline: bool,
        job: String,
    },
    /// Run a registry command on the window thread at the next frame boundary.
    Command(String, crate::shell::commands::Target),
}
/// Mirror effective settings and the human's settings undo labels into navigation.
pub(crate) fn sync_settings(state: &mut EditorState, settings: &crate::settings::Settings) {
    state.settings = settings.snapshot();
    state.settings_undo = settings.next_label(&ara_ops::Author::Human, false);
    state.settings_redo = settings.next_label(&ara_ops::Author::Human, true);
    state.apply_settings();
}
type R = Result<Value, Vec<Diagnostic>>;
impl Service for EditorService {
    fn name(&self) -> &'static str {
        "editor"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        use ara_host::params::SessionParams;
        let methods = vec![
            Method::new::<EditorProfileFreeze>(
                db,
                "editor.profile.freeze",
                "Freeze and save the current profiler trace for frame inspection or comparison.",
            ),
            Method::new::<EditorCanvas>(
                db,
                "editor.canvas",
                "Set graph pan, zoom, selection and search-to-add visibility.",
            ),
            Method::new::<EditorWorkspace>(
                db,
                "editor.workspace",
                "Select assets, folders and authoring documents, or switch thumbnail/list presentation.",
            ),
            Method::new::<EditorPreferences>(
                db,
                "editor.preferences",
                "Set reduced motion and Scene rendering quality.",
            ),
            Method::new::<EditorFollow>(
                db,
                "editor.follow",
                "Follow an agent's selection and camera; absent agent stops following.",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.history",
                "Read attributed human/agent navigation commands and their ops.",
            )
            .read_only(),
            Method::new::<EditorReport>(
                db,
                "editor.report",
                "Record a native editor diagnostic on the diagnostics bus.",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.state",
                "Read editor selection, camera and layout.",
            )
            .read_only(),
            Method::new::<SessionParams>(
                db,
                "editor.frame",
                "Frame the current selection, or the scene when empty.",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.layout.reset",
                "Restore the project's default docking layout.",
            ),
            Method::new::<EditorSelect>(
                db,
                "editor.select",
                "Select stable entity addresses; navigation has independent undo.",
            ),
            Method::new::<EditorFocus>(
                db,
                "editor.focus",
                "Activate a panel and optionally reveal a schema field.",
            ),
            Method::new::<EditorCamera>(db, "editor.camera", "Set the Scene view's orbit camera."),
            Method::new::<EditorSceneOpen>(
                db,
                "editor.scene.open",
                "Open a scene in the Scene view by project path (`levels/hangar.scene`) or document ID. A human's view switches (each scene keeps its camera; a first open frames the scene). An agent opens it in its own presence and the human gets a View/Follow notice; the human's view never moves by itself.",
            ),
            Method::new::<EditorTheme>(
                db,
                "editor.theme",
                "Switch Ara Dark / Ara Light semantic tokens.",
            ),
            Method::new::<EditorLayout>(
                db,
                "editor.layout",
                "Move/activate a panel or resize a dock split.",
            ),
            Method::new::<EditorTool>(
                db,
                "editor.tool",
                "Configure translate/rotate/scale handles and snapping.",
            ),
            Method::new::<EditorEntityView>(
                db,
                "editor.visibility",
                "Hide an entity in the edit viewport.",
            ),
            Method::new::<EditorEntityView>(
                db,
                "editor.lock",
                "Lock an entity against accidental pointer selection.",
            ),
            Method::new::<EditorEntityView>(
                db,
                "editor.collapse",
                "Collapse or expand a hierarchy subtree.",
            ),
            Method::new::<EditorSearch>(db, "editor.search", "Search a panel's indexed model."),
            Method::new::<EditorPalette>(
                db,
                "editor.palette",
                "Open or dismiss the command palette.",
            ),
            // No "f…r" sequence in name or summary: the palette golden searches "fr".
            Method::new::<EditorPlay>(
                db,
                "editor.play",
                "Start windowed Play: the listen host plus N client windows (D19), each behind an optional simulated link.",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.commands",
                "List every editor command: id, human title, menu path, shortcuts and why it is disabled.",
            )
            .read_only(),
            Method::new::<EditorCommand>(
                db,
                "editor.command",
                "Run an editor command by id at the next frame boundary, as a menu or shortcut would.",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.settings",
                "List project settings and preferences with values, defaults, scopes and descriptions.",
            )
            .read_only(),
            Method::new::<EditorSettingsSet>(
                db,
                "editor.settings.set",
                "Set or reset a setting as an undoable, attributed edit; project scope writes ara.project.json.",
            ),
            Method::new::<EditorSettingsUndo>(
                db,
                "editor.settings.undo",
                "Undo or redo the session's latest settings edit.",
            ),
            Method::new::<EditorSettingsView>(
                db,
                "editor.settings.view",
                "Choose the Settings window's scope, category and modified-only filter.",
            ),
            Method::new::<EditorView>(
                db,
                "editor.view",
                "Set the viewport debug view, game view, overlays, Select tool and maximized panel.",
            ),
            Method::new::<EditorLayoutName>(
                db,
                "editor.layout.apply",
                "Apply a built-in or saved layout, including floating windows.",
            ),
            Method::new::<EditorLayoutName>(
                db,
                "editor.layout.save",
                "Save the current layout under .ara/layouts.",
            ),
            Method::new::<EditorLayoutPanel>(
                db,
                "editor.layout.show",
                "Show a panel where it is, or open it in its home slot.",
            ),
            Method::new::<EditorLayoutPanel>(
                db,
                "editor.layout.close",
                "Close a panel in any window; reopen it with editor.layout.show.",
            ),
            Method::new::<EditorLayoutPanel>(
                db,
                "editor.layout.move",
                "Move a panel to left, right, bottom, view, or float (its own OS window at rect).",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.layout.next",
                "Activate the next tab in the document area.",
            ),
            Method::new::<EditorWindow>(
                db,
                "editor.window",
                "Record a floating window's desktop rectangle after the OS moved or resized it.",
            ),
        ];
        methods
            .into_iter()
            .map(|method| {
                let action = method.name.clone();
                method.action("ara_session", &action)
            })
            .collect()
    }
    fn call(&mut self, cx: &mut Cx<'_>, method: &str, params: Value) -> R {
        let result = self.dispatch(cx, method, params);
        // Preferences that could not be written are warnings on the bus, not stderr.
        for failure in self.settings.take_failures() {
            cx.log(
                Diagnostic::warning(crate::codes::SETTINGS, failure)
                    .subject(ara_diag::Subject::system("editor")),
            );
        }
        result
    }
}
impl EditorService {
    fn dispatch(&mut self, cx: &mut Cx<'_>, method: &str, params: Value) -> R {
        if method == "editor.state" {
            return Ok(json!({"state":self.navigation.state}));
        }
        if method == "editor.history" {
            return Ok(json!({"items":self.navigation.history()}));
        }
        if method == "editor.play" {
            // Resolves the `play.start` params; `Editor::action` starts the world.
            let p: EditorPlay = parse(params)?;
            let clients = p.clients.unwrap_or(1).min(16);
            let link =
                (p.latency_ms.is_some() || p.jitter_ms.is_some() || p.loss.is_some()).then(|| {
                    json!({"latencyMs": p.latency_ms.unwrap_or(0),
                           "jitterMs": p.jitter_ms.unwrap_or(0), "loss": p.loss.unwrap_or(0)})
                });
            return Ok(json!({
                "window": true,
                "mode": if p.dedicated.unwrap_or(false) { "dedicated" } else { "host" },
                "clients": clients,
                "conditions": vec![link; clients as usize],
            }));
        }
        if method == "editor.commands" {
            use crate::shell::commands::{Ctx, Target, export, keymap};
            let state = &self.navigation.state;
            let target = Target::None;
            let cx = Ctx {
                state,
                store: cx.store(),
                target: &target,
                clipboard_entities: false,
                clipboard_value: false,
                logs: crate::shell::LogView::default(),
                assets_show_all: false,
            };
            return Ok(json!({"commands": export(&cx, &keymap(state))}));
        }
        if method == "editor.command" {
            let p: EditorCommand = parse(params)?;
            if crate::shell::commands::get(&p.id).is_none() {
                return Err(vec![crate::invalid(format!(
                    "no command `{}`; see editor.commands",
                    p.id
                ))]);
            }
            let target = match p.target {
                None => crate::shell::commands::Target::None,
                Some(v) => serde_json::from_value(v)
                    .map_err(|e| vec![crate::invalid(format!("invalid target: {e}"))])?,
            };
            self.controls
                .send(Control::Command(p.id, target))
                .map_err(|_| vec![crate::invalid("editor window has closed")])?;
            return Ok(json!({"queued": true}));
        }
        if method == "editor.settings" {
            use crate::settings::{DEFS, Kind};
            let items: Vec<_> = DEFS
                .iter()
                .map(|d| {
                    let (control, options, range) = match d.kind {
                        Kind::Toggle => ("toggle", Value::Null, Value::Null),
                        Kind::Choice(o) => ("choice", json!(o.iter().map(|(v, l)| json!({"value": v, "label": l})).collect::<Vec<_>>()), Value::Null),
                        Kind::Number { min, max, step, unit } => ("number", Value::Null, json!({"min": min, "max": max, "step": step, "unit": unit})),
                        Kind::Text => ("text", Value::Null, Value::Null),
                        Kind::Map => ("map", Value::Null, Value::Null),
                    };
                    json!({"key": d.key, "scope": d.scope, "category": d.category, "label": d.label, "description": d.description, "control": control, "options": options, "range": range, "value": self.settings.value(d.key), "modified": self.settings.modified(d.key)})
                })
                .collect();
            return Ok(json!({"settings": items}));
        }
        if method == "editor.report" {
            let p: EditorReport = parse(params)?;
            let (code, warning) = match p.kind.as_deref().unwrap_or("action") {
                "thumbnail" => (crate::codes::THUMBNAIL, true),
                "settings" => (crate::codes::SETTINGS, true),
                "transport" => (crate::codes::TRANSPORT, false),
                "render" => (crate::codes::RENDER, false),
                _ => (crate::codes::ACTION, false),
            };
            let diagnostic = if warning {
                Diagnostic::warning(code, p.message)
            } else {
                Diagnostic::error(code, p.message)
            };
            let subject = p
                .subject
                .as_deref()
                .and_then(ara_diag::Subject::parse)
                .unwrap_or_else(|| ara_diag::Subject::system("editor"));
            cx.log(diagnostic.subject(subject));
            return Ok(json!({"ok":true}));
        }
        if method == "editor.profile.freeze" {
            let p: EditorProfileFreeze = parse(params)?;
            let job = cx.start_job("Save profiler capture");
            self.controls
                .send(Control::ProfileFreeze {
                    baseline: p.baseline.unwrap_or(false),
                    job: job.clone(),
                })
                .map_err(|_| vec![crate::invalid("editor window has closed")])?;
            return Ok(json!({"job":job}));
        }
        let mut state = self.navigation.state.clone();
        let agent = cx.session().and_then(|s| {
            matches!(s.author, ara_ops::Author::Agent(_))
                .then(|| (s.id.clone(), s.author.to_string()))
        });
        let separate = agent.is_some()
            && matches!(
                method,
                "editor.select" | "editor.camera" | "editor.frame" | "editor.scene.open"
            );
        let human = separate.then(|| (state.doc.clone(), state.selection.clone(), state.camera));
        if separate
            && let Some((id, _)) = &agent
            && let Some(presence) = state.presence.get(id)
        {
            state.doc = presence.doc.clone();
            state.selection = presence.selection.clone();
            state.camera = presence.camera;
        }
        match method {
            "editor.canvas" => {
                let p: EditorCanvas = parse(params)?;
                if let Some(pan) = p.pan {
                    if pan.iter().any(|v| !v.is_finite()) {
                        return Err(vec![crate::invalid("pan must be finite")]);
                    }
                    state.canvas.pan = pan;
                }
                if let Some(zoom) = p.zoom {
                    if !zoom.is_finite() || !(0.2..=2.5).contains(&zoom) {
                        return Err(vec![crate::invalid("zoom outside 0.2..2.5")]);
                    }
                    state.canvas.zoom = zoom;
                }
                if let Some(selection) = p.selection {
                    state.canvas_selection = selection.into_iter().collect();
                }
                if let Some(add) = p.add {
                    state.canvas_add = add;
                }
            }
            "editor.workspace" => {
                let p: EditorWorkspace = parse(params)?;
                if let Some(path) = p.asset {
                    state.asset_path = (!path.is_empty()).then_some(path);
                }
                if let Some(folder) = p.folder {
                    state.asset_folder = folder;
                }
                if let Some(grid) = p.grid {
                    state.asset_grid = grid;
                }
                if let Some(frame) = p.frame {
                    state.profiler_frame = Some(frame);
                }
                if let Some(compare) = p.compare {
                    state.profiler_compare = compare;
                }
                if let Some(playing) = p.playing {
                    state.preview_playing = playing;
                }
                if let Some(plane) = p.plane {
                    state.preview_plane = plane;
                }
                if let Some(blends) = p.blends {
                    state.animation_blends = blends;
                }
                if let Some(item) = p.item {
                    state.resource_item = (!item.is_empty()).then_some(item);
                }
                if let Some(resource) = p.resource {
                    let id = cx
                        .store()
                        .workspace()
                        .find(&resource)
                        .ok_or_else(|| vec![crate::invalid("resource document not found")])?;
                    state.resource = Some(id.to_string());
                    let doc = cx
                        .store()
                        .workspace()
                        .doc(id.as_str())
                        .ok_or_else(|| vec![crate::invalid("resource missing")])?;
                    let panel = if doc.kind == "animation" {
                        "animation"
                    } else {
                        doc.record("g_meta")
                            .and_then(|r| r.get("domain"))
                            .and_then(Value::as_str)
                            .unwrap_or("material")
                    };
                    if matches!(panel, "material" | "vfx" | "animation") {
                        let mut dock = state.dock().map_err(ui_error)?;
                        crate::state::show_panel(&mut dock, panel).map_err(ui_error)?;
                        state.set_dock(&dock);
                        state.focus = panel.into();
                    }
                    state.canvas = Default::default();
                    state.canvas_selection.clear();
                }
            }
            "editor.preferences" => {
                let p: EditorPreferences = parse(params)?;
                let author = author(cx);
                if let Some(v) = p.reduce_motion {
                    self.settings
                        .set("appearance.reduceMotion", Some(json!(v)), author.clone())?;
                }
                if let Some(v) = p.quality {
                    let quality = if v { "fidelity" } else { "raster" };
                    self.settings
                        .set("viewport.quality", Some(json!(quality)), author)?;
                }
                sync_settings(&mut state, &self.settings);
            }
            "editor.settings.set" => {
                let p: EditorSettingsSet = parse(params)?;
                let value = if p.reset.unwrap_or(false) {
                    None
                } else {
                    Some(
                        p.value
                            .ok_or_else(|| vec![crate::invalid("give a value, or reset: true")])?,
                    )
                };
                self.settings.set(&p.key, value, author(cx))?;
                sync_settings(&mut state, &self.settings);
            }
            "editor.settings.undo" => {
                let p: EditorSettingsUndo = parse(params)?;
                self.settings.undo(&author(cx), p.redo.unwrap_or(false))?;
                sync_settings(&mut state, &self.settings);
            }
            "editor.settings.view" => {
                let p: EditorSettingsView = parse(params)?;
                if let Some(scope) = p.scope {
                    if !matches!(scope.as_str(), "user" | "project") {
                        return Err(vec![crate::invalid("scope is `user` or `project`")]);
                    }
                    state.settings_scope = scope;
                }
                if let Some(category) = p.category {
                    state.settings_category = category;
                }
                if let Some(modified) = p.modified {
                    state.settings_modified = modified;
                }
            }
            "editor.view" => {
                let p: EditorView = parse(params)?;
                if let Some(mode) = p.view_mode {
                    if !matches!(
                        mode.as_str(),
                        "lit"
                            | "baseColor"
                            | "normals"
                            | "depth"
                            | "roughnessMetal"
                            | "entityIds"
                            | "shadowCascades"
                    ) {
                        return Err(vec![crate::invalid(format!("unknown view mode `{mode}`"))]);
                    }
                    state.view_mode = mode;
                }
                if let Some(v) = p.game_view {
                    state.game_view = v;
                }
                if let Some(v) = p.hide_overlays {
                    state.hide_overlays = v;
                }
                if let Some(v) = p.gizmos {
                    state.hide_gizmos = !v;
                }
                if let Some(v) = p.orientation {
                    state.hide_orientation = !v;
                }
                if let Some(panel) = p.maximized {
                    state.maximized = (!panel.is_empty()).then_some(panel);
                }
                if let Some(play) = p.play {
                    state.playing = match play.as_str() {
                        "playing" => Some(true),
                        "paused" => Some(false),
                        "stopped" => None,
                        _ => {
                            return Err(vec![crate::invalid("play is playing, paused or stopped")]);
                        }
                    };
                }
            }
            "editor.layout.apply" => {
                let p: EditorLayoutName = parse(params)?;
                let dock = if let Some(root) = crate::state::preset_layout(&p.name) {
                    DockLayout::new(root).map_err(ui_error)?
                } else {
                    let path = cx
                        .root()
                        .join(".ara/layouts")
                        .join(format!("{}.json", safe_name(&p.name)?));
                    let text = std::fs::read_to_string(&path)
                        .map_err(|_| vec![crate::invalid(format!("no layout `{}`", p.name))])?;
                    let mut dock = DockLayout::new(default_layout()).map_err(ui_error)?;
                    dock.restore(&text).map_err(ui_error)?;
                    dock
                };
                state.set_dock(&dock);
                state.maximized = None;
            }
            "editor.layout.save" => {
                let p: EditorLayoutName = parse(params)?;
                let path = cx
                    .root()
                    .join(".ara/layouts")
                    .join(format!("{}.json", safe_name(&p.name)?));
                crate::state::write_layout(&path, &state)
                    .map_err(|e| vec![crate::invalid(e.to_string())])?;
                return Ok(json!({"saved": path}));
            }
            "editor.layout.show" | "editor.layout.close" | "editor.layout.move" => {
                let p: EditorLayoutPanel = parse(params)?;
                let mut dock = state.dock().map_err(ui_error)?;
                match method {
                    "editor.layout.show" => {
                        if p.panel == "settings" && !dock.contains("settings") {
                            dock_float(
                                &mut dock,
                                &p.panel,
                                p.rect.map(rect).unwrap_or([160, 120, 960, 640]),
                            )?;
                        }
                        crate::state::show_panel(&mut dock, &p.panel).map_err(ui_error)?;
                        state.focus = p.panel.clone();
                    }
                    "editor.layout.close" => {
                        if p.panel == "scene" {
                            return Err(vec![crate::invalid("the Scene view cannot be closed")]);
                        }
                        dock.close(&p.panel).map_err(ui_error)?;
                        if state.maximized.as_deref() == Some(p.panel.as_str()) {
                            state.maximized = None;
                        }
                    }
                    _ => match p.slot.as_deref() {
                        Some("float") => {
                            if p.panel == "scene" {
                                return Err(vec![crate::invalid(
                                    "the Scene viewport stays in the main window",
                                )]);
                            }
                            dock_float(
                                &mut dock,
                                &p.panel,
                                p.rect.map(rect).unwrap_or([200, 160, 420, 560]),
                            )?;
                        }
                        Some(slot @ ("left" | "right" | "bottom" | "view")) => {
                            crate::state::move_panel(&mut dock, &p.panel, slot)
                                .map_err(ui_error)?;
                            dock.activate(&p.panel).map_err(ui_error)?;
                        }
                        _ => {
                            return Err(vec![crate::invalid(
                                "slot is left, right, bottom, view or float",
                            )]);
                        }
                    },
                }
                state.set_dock(&dock);
            }
            "editor.layout.next" => {
                fn next(n: &mut DockNode) -> bool {
                    match n {
                        DockNode::Area { id, tabs, active } if id == "view" && !tabs.is_empty() => {
                            *active = (*active + 1) % tabs.len();
                            true
                        }
                        DockNode::Split { first, second, .. } => next(first) || next(second),
                        _ => false,
                    }
                }
                next(&mut state.layout);
            }
            "editor.window" => {
                let p: EditorWindow = parse(params)?;
                let mut dock = state.dock().map_err(ui_error)?;
                dock.set_rect(&p.id, rect(p.rect)).map_err(ui_error)?;
                state.set_dock(&dock);
            }
            "editor.follow" => {
                let p: EditorFollow = parse(params)?;
                if agent.is_some() {
                    return Err(vec![crate::invalid(
                        "only a human chooses which agent to follow",
                    )]);
                }
                if let Some(id) = &p.agent {
                    let presence = state.presence.get(id).ok_or_else(|| {
                        vec![crate::invalid("agent has not published presence yet")]
                    })?;
                    state.doc = presence.doc.clone();
                    state.selection = presence.selection.clone();
                    state.camera = presence.camera;
                }
                state.follow = p.agent;
            }
            "editor.select" => {
                state.asset_path = None;
                let p: EditorSelect = parse(params)?;
                if !separate {
                    state.focus_field = None;
                }
                if let Some(doc) = p.doc {
                    let d = cx
                        .store()
                        .workspace()
                        .find(&doc)
                        .ok_or_else(|| vec![crate::invalid("document not found")])?
                        .to_string();
                    open_scene(&mut state, cx, d, agent.is_none())?;
                }
                let resolved = crate::world::resolve(cx.store(), &state.doc)?;
                for entity in &p.entities {
                    if !resolved.contains_key(entity) {
                        return Err(vec![crate::invalid(format!("no entity `{entity}`"))]);
                    }
                }
                if !p.toggle.unwrap_or(false) && !p.additive.unwrap_or(false) {
                    state.selection.clear();
                }
                for entity in p.entities {
                    if p.toggle.unwrap_or(false) && state.selection.contains(&entity) {
                        state.selection.retain(|e| e != &entity);
                    } else if !state.selection.contains(&entity) {
                        state.selection.push(entity);
                    }
                }
            }
            "editor.focus" => {
                let p: EditorFocus = parse(params)?;
                let mut dock = state.dock().map_err(ui_error)?;
                crate::state::show_panel(&mut dock, &p.panel).map_err(ui_error)?;
                state.set_dock(&dock);
                state.focus = p.panel;
                state.focus_field = p.field;
            }
            "editor.camera" => {
                let p: EditorCamera = parse(params)?;
                if !p.distance.is_finite()
                    || p.distance <= 0.
                    || !p.yaw.is_finite()
                    || !p.pitch.is_finite()
                    || p.target.iter().any(|x| !x.is_finite())
                {
                    return Err(vec![crate::invalid(
                        "camera values must be finite; distance must be positive",
                    )]);
                }
                state.camera = crate::state::Camera {
                    target: p.target.into(),
                    yaw: p.yaw,
                    pitch: p.pitch.clamp(-1.54, 1.54),
                    distance: p.distance.clamp(0.05, 100_000.),
                };
            }
            "editor.frame" => {
                frame(&mut state, cx)?;
            }
            "editor.scene.open" => {
                let p: EditorSceneOpen = parse(params)?;
                let workspace = cx.store().workspace();
                let id = workspace
                    .find(&p.scene)
                    .ok_or_else(|| {
                        vec![crate::invalid(format!(
                            "no document `{}`; use a project path like `levels/main.scene`",
                            p.scene
                        ))]
                    })?
                    .to_string();
                if workspace
                    .doc(&id)
                    .is_none_or(|d| !matches!(d.kind.as_str(), "scene" | "prefab"))
                {
                    return Err(vec![crate::invalid(format!(
                        "`{}` is not a scene or prefab",
                        p.scene
                    ))]);
                }
                open_scene(&mut state, cx, id, agent.is_none())?;
            }
            "editor.theme" => {
                let p: EditorTheme = parse(params)?;
                let theme = if p.light { "light" } else { "dark" };
                self.settings
                    .set("appearance.theme", Some(json!(theme)), author(cx))?;
                sync_settings(&mut state, &self.settings);
            }
            "editor.layout.reset" => {
                state.set_dock(&DockLayout::new(default_layout()).map_err(ui_error)?);
                state.maximized = None;
            }
            "editor.layout" => {
                let p: EditorLayout = parse(params)?;
                let mut dock = state.dock().map_err(ui_error)?;
                if let Some(ratio) = p.ratio {
                    dock.resize_split(p.path.as_deref().unwrap_or(&[]), ratio)
                        .map_err(ui_error)?;
                } else if let Some(panel) = p.panel {
                    state.focus = panel.clone();
                    if let Some(area) = p.area {
                        let target = if let Some(new_area) = p.new_area {
                            DockDrop::Split {
                                area,
                                new_area,
                                axis: if p.vertical.unwrap_or(false) {
                                    Axis::Vertical
                                } else {
                                    Axis::Horizontal
                                },
                                before: p.before.unwrap_or(false),
                            }
                        } else {
                            DockDrop::Tab {
                                area,
                                index: p.index.unwrap_or(u32::MAX) as usize,
                            }
                        };
                        dock.drop_tab(&panel, target).map_err(ui_error)?;
                    } else {
                        dock.activate(&panel).map_err(ui_error)?;
                    }
                }
                state.set_dock(&dock);
            }
            "editor.tool" => {
                let p: EditorTool = parse(params)?;
                state.tool = p.tool;
                if let Some(v) = p.local {
                    state.local = v;
                }
                if let Some(v) = p.centre {
                    state.centre = v;
                }
                if let Some(v) = p.snapping {
                    state.snapping = v;
                }
                for (input, target) in [
                    (p.move_snap, &mut state.move_snap),
                    (p.rotate_snap, &mut state.rotate_snap),
                    (p.scale_snap, &mut state.scale_snap),
                ] {
                    if let Some(v) = input {
                        if !v.is_finite() || v <= 0. {
                            return Err(vec![crate::invalid(
                                "snap increments must be finite and positive",
                            )]);
                        }
                        *target = v;
                    }
                }
            }
            "editor.visibility" | "editor.lock" | "editor.collapse" => {
                let p: EditorEntityView = parse(params)?;
                let set = match method {
                    "editor.visibility" => &mut state.hidden,
                    "editor.lock" => &mut state.locked,
                    _ => &mut state.collapsed,
                };
                if p.enabled {
                    set.insert(p.entity);
                } else {
                    set.remove(&p.entity);
                }
            }
            "editor.search" => {
                let p: EditorSearch = parse(params)?;
                state.searches.insert(p.panel, p.query);
            }
            "editor.palette" => {
                let p: EditorPalette = parse(params)?;
                state.palette = p.open;
            }
            _ => return Err(vec![crate::invalid("unknown editor action")]),
        }
        if let Some((doc, selection, camera)) = human
            && let Some((id, author)) = agent
        {
            state.presence.insert(
                id.clone(),
                crate::state::AgentPresence {
                    author,
                    doc: state.doc.clone(),
                    selection: state.selection.clone(),
                    camera: state.camera,
                },
            );
            if state.follow.as_ref() != Some(&id) {
                state.doc = doc;
                state.selection = selection;
                state.camera = camera;
            }
        }
        let author = cx
            .session()
            .map(|s| s.author.clone())
            .unwrap_or(ara_ops::Author::Human);
        let result = self.navigation.commit(state, author, method, cx.root())?;
        self.changes
            .send(self.navigation.state.clone())
            .map_err(|_| vec![crate::invalid("editor window has closed")])?;
        cx.notify(ara_host::params::Topic::Ops, json!({"editor":result}));
        Ok(result)
    }
}
fn author(cx: &Cx<'_>) -> ara_ops::Author {
    cx.session()
        .map(|s| s.author.clone())
        .unwrap_or(ara_ops::Author::Human)
}

/// Protocol rectangles are numbers; floating windows use whole desktop pixels.
fn rect(r: [f32; 4]) -> [i32; 4] {
    r.map(|v| v.round() as i32)
}
/// Tear a panel off into a new floating window with a unique id.
fn dock_float(dock: &mut DockLayout, panel: &str, rect: [i32; 4]) -> Result<(), Vec<Diagnostic>> {
    let id = (1..)
        .map(|n| format!("window-{n}"))
        .find(|id| dock.floating().iter().all(|f| &f.id != id))
        .unwrap_or_default();
    if !dock.contains(panel) {
        // Closed panels float from nowhere: open them first, then move.
        crate::state::move_panel(dock, panel, "view").map_err(ui_error)?;
    }
    dock.float(panel, &id, rect).map_err(ui_error)
}
/// Layout names become file stems; keep them to letters, digits, spaces, `-` and `_`.
fn safe_name(name: &str) -> Result<&str, Vec<Diagnostic>> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
    {
        return Err(vec![crate::invalid(
            "layout names use letters, digits, spaces, - and _",
        )]);
    }
    Ok(name)
}
fn ui_error(error: Box<dyn std::error::Error>) -> Vec<Diagnostic> {
    vec![crate::invalid(error.to_string())]
}
/// Frame the selection (or the whole scene when nothing is selected).
fn frame(state: &mut EditorState, cx: &mut Cx) -> Result<(), Vec<Diagnostic>> {
    let resolved = crate::world::resolve(cx.store(), &state.doc)?;
    let transforms = crate::world::world_transforms(&resolved);
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (id, matrix) in transforms {
        if state.selection.is_empty() || state.selection.contains(&id) {
            for x in [-0.5, 0.5] {
                for y in [-0.5, 0.5] {
                    for z in [-0.5, 0.5] {
                        let p = matrix.transform_point3(Vec3::new(x, y, z));
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
            }
        }
    }
    if min.is_finite() {
        state.camera.target = (min + max) * 0.5;
        state.camera.distance = (max - min).length().max(1.) * 1.8;
    }
    Ok(())
}
/// Switch to scene `doc`: the outgoing scene keeps its camera, a reopened scene gets
/// its own back and a first open frames the whole scene. Humans (`human`) also update
/// the recent-scenes list; agents only move their presence.
fn open_scene(
    state: &mut EditorState,
    cx: &mut Cx,
    doc: String,
    human: bool,
) -> Result<(), Vec<Diagnostic>> {
    if state.doc != doc {
        state.scene_cameras.insert(state.doc.clone(), state.camera);
        state.doc = doc.clone();
        state.selection.clear();
        match state.scene_cameras.get(&doc) {
            Some(camera) => state.camera = *camera,
            None => frame(state, cx)?,
        }
    }
    if human {
        state.recent_scenes.retain(|d| *d != doc);
        state.recent_scenes.insert(0, doc);
        state.recent_scenes.truncate(10);
    }
    Ok(())
}
