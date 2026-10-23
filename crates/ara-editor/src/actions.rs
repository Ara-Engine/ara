//! Reflected editor actions. Each successful navigation command produces reversible ops.
use crate::state::{ApplyPolicy, EditorState, Navigation, Tool, default_layout};
use ara_diag::Diagnostic;
use ara_host::{Cx, Method, Service, parse};
use ara_proto::Scope;
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
    pub changes: Sender<EditorState>,
    pub controls: Sender<Control>,
}
pub(crate) enum Control {
    Approve(String),
    Reject(String),
    ProfileFreeze { baseline: bool, job: String },
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
            Method::new::<EditorPending>(
                db,
                "editor.pending.apply",
                "Approve a deferred agent mutation at the next frame boundary.",
            )
            .scope(Scope::EditScene),
            Method::new::<EditorPending>(
                db,
                "editor.pending.reject",
                "Reject a deferred mutation before it changes documents.",
            )
            .scope(Scope::EditScene),
            Method::new::<EditorReport>(
                db,
                "editor.report",
                "Record a native editor diagnostic on the diagnostics bus.",
            ),
            Method::new::<SessionParams>(
                db,
                "editor.state",
                "Read editor selection, camera, layout and policy.",
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
            Method::new::<EditorPolicy>(
                db,
                "editor.policy",
                "Choose when incoming agent changes apply.",
            )
            .scope(Scope::EditScene),
            Method::new::<EditorPalette>(
                db,
                "editor.palette",
                "Open or dismiss the command palette.",
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
        if method == "editor.state" {
            return Ok(json!({"state":self.navigation.state}));
        }
        if method == "editor.history" {
            return Ok(json!({"items":self.navigation.history()}));
        }
        if method == "editor.report" {
            let p: EditorReport = parse(params)?;
            cx.log(crate::invalid(p.message));
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
        if method.starts_with("editor.pending.") {
            let p: EditorPending = parse(params)?;
            if !cx
                .session()
                .is_some_and(|s| s.author == ara_ops::Author::Human)
            {
                return Err(vec![crate::invalid(
                    "a human reviewer must decide pending agent changes",
                )]);
            }
            let control = if method == "editor.pending.apply" {
                Control::Approve(p.ticket)
            } else {
                Control::Reject(p.ticket)
            };
            self.controls
                .send(control)
                .map_err(|_| vec![crate::invalid("editor window has closed")])?;
            return Ok(json!({"ok":true}));
        }
        let mut state = self.navigation.state.clone();
        let agent = cx.session().and_then(|s| {
            matches!(s.author, ara_ops::Author::Agent(_))
                .then(|| (s.id.clone(), s.author.to_string()))
        });
        let separate =
            agent.is_some() && matches!(method, "editor.select" | "editor.camera" | "editor.frame");
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
                        let mut dock = layout(&state.layout)?;
                        dock.activate(panel).map_err(ui_error)?;
                        state.layout = dock.root().clone();
                        state.focus = panel.into();
                    }
                    state.canvas = Default::default();
                    state.canvas_selection.clear();
                }
            }
            "editor.preferences" => {
                let p: EditorPreferences = parse(params)?;
                if let Some(v) = p.reduce_motion {
                    state.reduce_motion = v;
                }
                if let Some(v) = p.quality {
                    state.quality = v;
                }
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
                        .ok_or_else(|| vec![crate::invalid("document not found")])?;
                    state.doc = d.to_string();
                    state.selection.clear();
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
                let mut layout = layout(&state.layout)?;
                layout.activate(&p.panel).map_err(ui_error)?;
                state.layout = layout.root().clone();
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
            }
            "editor.theme" => {
                let p: EditorTheme = parse(params)?;
                state.light = p.light;
            }
            "editor.layout.reset" => state.layout = default_layout(),
            "editor.layout" => {
                let p: EditorLayout = parse(params)?;
                let mut dock = layout(&state.layout)?;
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
                state.layout = dock.root().clone();
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
            "editor.policy" => {
                let p: EditorPolicy = parse(params)?;
                if !cx
                    .session()
                    .is_some_and(|s| matches!(s.author, ara_ops::Author::Human))
                {
                    return Err(vec![crate::invalid(
                        "only a human session may set the apply policy",
                    )]);
                }
                state.policy = p.policy;
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
fn layout(root: &DockNode) -> Result<DockLayout, Vec<Diagnostic>> {
    DockLayout::new(root.clone()).map_err(ui_error)
}
fn ui_error(error: Box<dyn std::error::Error>) -> Vec<Diagnostic> {
    vec![crate::invalid(error.to_string())]
}
