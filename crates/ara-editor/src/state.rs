//! Navigation state is an op document with a history independent of scene undo.
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::{FieldPath, Schema, SchemaDb};
use ara_ui::docking::{Axis, DockDrop, DockLayout, DockNode, Floating};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// The D20 boundary at which an agent batch may change authoring documents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplyPolicy {
    /// Apply at the next editor frame boundary.
    #[default]
    Immediately,
    /// Queue while a human gesture or text edit is active.
    WhenIdle,
    /// Queue until a human approves the batch.
    OnApproval,
}
/// Editor transform handle mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    /// Move along one or several axes.
    #[default]
    Translate,
    /// Rotate around a chosen axis.
    Rotate,
    /// Scale along an axis or uniformly.
    Scale,
}
/// Perspective scene camera; this is editor state, never gameplay state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Camera {
    /// Orbit centre in metres.
    pub target: Vec3,
    /// Azimuth in radians.
    pub yaw: f32,
    /// Elevation in radians, bounded away from the poles.
    pub pitch: f32,
    /// Orbit radius in metres.
    pub distance: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::new(0., 0.5, 0.),
            yaw: 0.6,
            pitch: 0.4,
            distance: 10.,
        }
    }
}
impl Camera {
    /// Camera position, derived from its orbit representation.
    pub fn position(&self) -> Vec3 {
        self.target
            + Vec3::new(
                self.yaw.sin() * self.pitch.cos(),
                self.pitch.sin(),
                self.yaw.cos() * self.pitch.cos(),
            ) * self.distance
    }
    /// Renderer-neutral view request.
    pub fn spec(&self) -> ara_render::scene::CameraSpec {
        ara_render::scene::CameraSpec::LookAt {
            position: self.position(),
            target: self.target,
            vertical_fov: 50f32.to_radians(),
        }
    }
    /// Pan in the camera's image plane, in logical pixels.
    pub fn pan(&mut self, delta: [f32; 2]) {
        let right = Vec3::new(self.yaw.cos(), 0., -self.yaw.sin());
        let forward = (self.target - self.position()).normalize();
        let up = right.cross(forward);
        self.target += (-right * delta[0] + up * delta[1]) * self.distance * 0.0015;
    }
    /// Rotate without permitting upside-down orbit singularities.
    pub fn orbit(&mut self, delta: [f32; 2]) {
        self.yaw -= delta[0] * 0.005;
        self.pitch = (self.pitch + delta[1] * 0.005).clamp(-1.54, 1.54);
    }
    /// Exponential dolly, bounded to useful editor scales.
    pub fn zoom(&mut self, amount: f32) {
        self.distance = (self.distance * (-amount * 0.12).exp()).clamp(0.05, 100_000.);
    }
    /// Fly in view-local coordinates at a speed supplied by the input controller.
    pub fn fly(&mut self, direction: Vec3, metres: f32) {
        let rotation = Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(-self.pitch);
        self.target += rotation * direction * metres;
    }
}
/// Serializable editor navigation and dock state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresence {
    /// Attributed agent name.
    pub author: String,
    /// Agent's active document.
    pub doc: String,
    /// Agent's private selection.
    pub selection: Vec<String>,
    /// Agent's private scene camera.
    pub camera: Camera,
}
/// Serializable editor navigation and dock state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EditorState {
    /// Active scene's asset ID or project-relative path.
    pub doc: String,
    /// Stable selected entity addresses, in selection order.
    pub selection: Vec<String>,
    /// Panel receiving keyboard commands.
    pub focus: String,
    /// A field to reveal in the inspector.
    pub focus_field: Option<String>,
    /// Current dock tree, persisted per project.
    pub layout: DockNode,
    /// Use Ara Light instead of the default Ara Dark.
    pub light: bool,
    /// Disable decorative transitions while preserving all interaction feedback.
    pub reduce_motion: bool,
    /// Use the same-device fidelity view for Scene lighting.
    pub quality: bool,
    /// View pose.
    pub camera: Camera,
    /// Transform handle type.
    pub tool: Tool,
    /// Transform axes follow entity orientation.
    pub local: bool,
    /// Use the common selection centre instead of each object's pivot.
    pub centre: bool,
    /// Snap enabled for the current tool.
    pub snapping: bool,
    /// Translation increment in metres.
    pub move_snap: f32,
    /// Rotation increment in degrees.
    pub rotate_snap: f32,
    /// Scale increment.
    pub scale_snap: f32,
    /// When to apply incoming agent batches.
    pub policy: ApplyPolicy,
    /// Collapsed hierarchy addresses.
    pub collapsed: BTreeSet<String>,
    /// Entities hidden in the scene viewport only.
    pub hidden: BTreeSet<String>,
    /// Soft editor selection locks; no effect on agent command permissions.
    pub locked: BTreeSet<String>,
    /// Panel search text.
    pub searches: BTreeMap<String, String>,
    /// Command palette visibility.
    pub palette: bool,
    /// Per-session agent selections and cameras, independent of the human's view.
    pub presence: BTreeMap<String, AgentPresence>,
    /// Session whose view the human explicitly chose to follow.
    pub follow: Option<String>,
    /// Selected source media path; separate from entity selection.
    pub asset_path: Option<String>,
    /// Folder shown by the asset browser, relative to the project root.
    pub asset_folder: String,
    /// Show thumbnail cards instead of compact asset rows.
    pub asset_grid: bool,
    /// Active material, effect or animation document identity.
    pub resource: Option<String>,
    /// Pan/zoom for the shared authoring canvas.
    pub canvas: crate::graph::Camera,
    /// Stable selected graph nodes, independent of scene selection.
    pub canvas_selection: BTreeSet<String>,
    /// Search-to-add palette visibility.
    pub canvas_add: bool,
    /// Selected module or controller state in the active authoring resource.
    pub resource_item: Option<String>,
    /// Frame selected in the profiler's frozen or live timeline.
    pub profiler_frame: Option<u64>,
    /// Show capture deltas instead of a single-frame detail.
    pub profiler_compare: bool,
    /// Show the animation pose/blend-space graph instead of the state machine.
    pub animation_blends: bool,
    /// Run fixed ticks in the live effect preview.
    pub preview_playing: bool,
    /// Render material previews on a plane.
    pub preview_plane: bool,
    /// Panels torn off into their own OS windows; persisted with the layout.
    pub floating: Vec<Floating>,
    /// Effective settings of both scopes, mirrored from `editor.settings`.
    pub settings: BTreeMap<String, Value>,
    /// Label of the human's next settings undo, if any.
    pub settings_undo: Option<String>,
    /// Label of the human's next settings redo, if any.
    pub settings_redo: Option<String>,
    /// Settings window scope: `user` (Preferences) or `project`.
    pub settings_scope: String,
    /// Settings window category; empty shows every category.
    pub settings_category: String,
    /// Settings window lists only values that differ from their defaults.
    pub settings_modified: bool,
    /// Live viewport debug view: `lit`, `baseColor`, `normals`, `depth`, …
    pub view_mode: String,
    /// Hide every editor visual in the viewport (G).
    pub game_view: bool,
    /// Hide the viewport toolbar and widgets (Shift+G).
    pub hide_overlays: bool,
    /// Select tool: no transform handles on the selection.
    pub hide_gizmos: bool,
    /// Hide the orientation widget.
    pub hide_orientation: bool,
    /// Panel filling the main window (Shift+Space), if any.
    pub maximized: Option<String>,
    /// Play session: `None` stopped, `Some(true)` running, `Some(false)` paused.
    pub playing: Option<bool>,
}
impl Default for EditorState {
    fn default() -> Self {
        Self {
            doc: String::new(),
            selection: Vec::new(),
            focus: "scene".into(),
            focus_field: None,
            layout: default_layout(),
            light: false,
            reduce_motion: false,
            quality: true,
            camera: Camera::default(),
            tool: Tool::Translate,
            local: false,
            centre: false,
            snapping: false,
            move_snap: 0.5,
            rotate_snap: 15.,
            scale_snap: 0.1,
            policy: ApplyPolicy::Immediately,
            collapsed: BTreeSet::new(),
            hidden: BTreeSet::new(),
            locked: BTreeSet::new(),
            searches: BTreeMap::new(),
            palette: false,
            presence: BTreeMap::new(),
            follow: None,
            asset_path: None,
            asset_folder: String::new(),
            asset_grid: true,
            resource: None,
            canvas: Default::default(),
            canvas_selection: BTreeSet::new(),
            canvas_add: false,
            resource_item: None,
            profiler_frame: None,
            profiler_compare: false,
            animation_blends: false,
            preview_playing: true,
            preview_plane: false,
            floating: Vec::new(),
            settings: BTreeMap::new(),
            settings_undo: None,
            settings_redo: None,
            settings_scope: "user".into(),
            settings_category: String::new(),
            settings_modified: false,
            view_mode: "lit".into(),
            game_view: false,
            hide_overlays: false,
            hide_gizmos: false,
            hide_orientation: false,
            maximized: None,
            playing: None,
        }
    }
}
impl EditorState {
    pub(crate) fn panels_changed(&self, other: &Self) -> bool {
        self.doc != other.doc
            || self.asset_path != other.asset_path
            || self.asset_folder != other.asset_folder
            || self.asset_grid != other.asset_grid
            || self.resource != other.resource
            || self.canvas_add != other.canvas_add
            || self.resource_item != other.resource_item
            || self.profiler_frame != other.profiler_frame
            || self.profiler_compare != other.profiler_compare
            || self.animation_blends != other.animation_blends
            || self.selection != other.selection
            || self.focus != other.focus
            || self.focus_field != other.focus_field
            || self.layout != other.layout
            || self.light != other.light
            || self.reduce_motion != other.reduce_motion
            || self.quality != other.quality
            || self.tool != other.tool
            || self.local != other.local
            || self.centre != other.centre
            || self.snapping != other.snapping
            || self.policy != other.policy
            || self.collapsed != other.collapsed
            || self.hidden != other.hidden
            || self.locked != other.locked
            || self.searches != other.searches
            || self.palette != other.palette
            || self.presence != other.presence
            || self.follow != other.follow
            || self.floating != other.floating
            || self.settings != other.settings
            || self.settings_undo != other.settings_undo
            || self.settings_redo != other.settings_redo
            || self.settings_scope != other.settings_scope
            || self.settings_category != other.settings_category
            || self.settings_modified != other.settings_modified
            || self.view_mode != other.view_mode
            || self.game_view != other.game_view
            || self.hide_overlays != other.hide_overlays
            || self.hide_gizmos != other.hide_gizmos
            || self.hide_orientation != other.hide_orientation
            || self.maximized != other.maximized
            || self.playing != other.playing
    }
    /// Mirror settings that older navigation fields expose.
    pub(crate) fn apply_settings(&mut self) {
        let text = |key: &str| {
            self.settings
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        if let Some(theme) = text("appearance.theme") {
            self.light = theme == "light";
        }
        if let Some(quality) = text("viewport.quality") {
            self.quality = quality == "fidelity";
        }
        if let Some(policy) = text("agents.applyPolicy") {
            self.policy = match policy.as_str() {
                "whenIdle" => ApplyPolicy::WhenIdle,
                "onApproval" => ApplyPolicy::OnApproval,
                _ => ApplyPolicy::Immediately,
            };
        }
        if let Some(v) = self
            .settings
            .get("appearance.reduceMotion")
            .and_then(Value::as_bool)
        {
            self.reduce_motion = v;
        }
    }
    /// A setting's number, or `default`.
    pub fn number(&self, key: &str, default: f64) -> f64 {
        self.settings
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(default)
    }
    /// Comfortable density adds 4 px to rows and controls.
    pub fn comfortable(&self) -> bool {
        self.settings
            .get("appearance.density")
            .and_then(Value::as_str)
            == Some("comfortable")
    }
    /// The validated dock model for the main tree and floating windows.
    pub fn dock(&self) -> ara_ui::Result<DockLayout> {
        DockLayout::with_floating(self.layout.clone(), self.floating.clone())
    }
    /// Store a dock model back into navigation state. Focus returns to the Scene
    /// when the focused panel is no longer open.
    pub fn set_dock(&mut self, dock: &DockLayout) {
        self.layout = dock.root().clone();
        self.floating = dock.floating().to_vec();
        if !dock.contains(&self.focus) {
            self.focus = "scene".into();
        }
    }
}
/// Whether a panel is open in the main window or a floating window.
pub fn panel_visible(state: &EditorState, panel: &str) -> bool {
    state.dock().is_ok_and(|d| d.contains(panel))
}
/// The area a panel returns to when shown from the Window menu.
fn home(panel: &str) -> &'static str {
    match panel {
        "hierarchy" => "left",
        "inspector" => "right",
        "assets" | "console" | "agents" => "bottom",
        _ => "view",
    }
}
/// Show a panel: activate it where it is, or open it in its home slot.
pub fn show_panel(dock: &mut DockLayout, panel: &str) -> ara_ui::Result<()> {
    if !dock.contains(panel) {
        move_panel(dock, panel, home(panel))?;
    }
    dock.activate(panel)
}
/// Dock a panel into a slot (`left`, `right`, `bottom`, `view`), creating the
/// slot's column by splitting the main window when it does not exist.
pub fn move_panel(dock: &mut DockLayout, panel: &str, slot: &str) -> ara_ui::Result<()> {
    fn has_area(n: &DockNode, id: &str) -> bool {
        match n {
            DockNode::Area { id: a, .. } => a == id,
            DockNode::Split { first, second, .. } => has_area(first, id) || has_area(second, id),
        }
    }
    fn first_area(n: &DockNode) -> String {
        match n {
            DockNode::Area { id, .. } => id.clone(),
            DockNode::Split { first, .. } => first_area(first),
        }
    }
    let target = if has_area(dock.root(), slot) {
        slot.to_owned()
    } else if slot == "view" {
        first_area(dock.root())
    } else {
        String::new()
    };
    if !target.is_empty() {
        if dock.contains(panel) {
            dock.drop_tab(
                panel,
                DockDrop::Tab {
                    area: target,
                    index: usize::MAX,
                },
            )?;
        } else {
            add_tab(dock, &target, panel)?;
        }
        return Ok(());
    }
    // Split the whole main tree so the new column spans the window.
    let mut root = dock.root().clone();
    let floating = dock.floating().to_vec();
    let mut fresh = DockLayout::with_floating(root.clone(), floating.clone())?;
    if fresh.contains(panel) {
        fresh.close(panel)?;
        root = fresh.root().clone();
    }
    let added = DockNode::Area {
        id: slot.into(),
        tabs: vec![panel.into()],
        active: 0,
    };
    let (axis, ratio, before) = match slot {
        "left" => (Axis::Horizontal, 0.18, true),
        "right" => (Axis::Horizontal, 0.78, false),
        _ => (Axis::Vertical, 0.74, false),
    };
    let (first, second) = if before { (added, root) } else { (root, added) };
    let root = DockNode::Split {
        axis,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    };
    *dock = DockLayout::with_floating(root, fresh.floating().to_vec())?;
    Ok(())
}
fn add_tab(dock: &mut DockLayout, area: &str, panel: &str) -> ara_ui::Result<()> {
    fn visit(n: &mut DockNode, area: &str, panel: &str) -> bool {
        match n {
            DockNode::Area { id, tabs, active } if id == area => {
                tabs.push(panel.into());
                *active = tabs.len() - 1;
                true
            }
            DockNode::Split { first, second, .. } => {
                visit(first, area, panel) || visit(second, area, panel)
            }
            _ => false,
        }
    }
    let mut root = dock.root().clone();
    let mut floating = dock.floating().to_vec();
    if !visit(&mut root, area, panel)
        && !floating.iter_mut().any(|f| visit(&mut f.root, area, panel))
    {
        return Err("ARA7042: dock destination missing".into());
    }
    *dock = DockLayout::with_floating(root, floating)?;
    Ok(())
}
/// The registered default arrangement; IDs are stable protocol and persistence keys.
pub fn default_layout() -> DockNode {
    preset_layout("default").unwrap_or_else(|| area("view", &["scene"]))
}
fn area(id: &str, tabs: &[&str]) -> DockNode {
    DockNode::Area {
        id: id.into(),
        tabs: tabs.iter().map(|s| s.to_string()).collect(),
        active: 0,
    }
}
fn split(axis: Axis, ratio: f32, first: DockNode, second: DockNode) -> DockNode {
    DockNode::Split {
        axis,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}
/// Built-in layouts (Window › Layouts). The viewport keeps the full window height
/// in every preset; Assets and Console share the left column under Hierarchy.
pub fn preset_layout(name: &str) -> Option<DockNode> {
    use Axis::{Horizontal as H, Vertical as V};
    const DOCUMENTS: &[&str] = &["scene", "game", "material", "vfx", "animation", "profiler"];
    Some(match name {
        "default" => split(
            H,
            0.17,
            split(
                V,
                0.5,
                area("left", &["hierarchy"]),
                area("bottom", &["assets", "console", "agents"]),
            ),
            split(
                H,
                0.76,
                area("view", DOCUMENTS),
                area("right", &["inspector"]),
            ),
        ),
        "levelDesign" => split(
            H,
            0.78,
            split(
                V,
                0.8,
                area("view", DOCUMENTS),
                area("bottom", &["assets", "console", "agents"]),
            ),
            split(
                V,
                0.42,
                area("left", &["hierarchy"]),
                area("right", &["inspector"]),
            ),
        ),
        "materials" => split(
            H,
            0.17,
            split(
                V,
                0.62,
                area("bottom", &["assets", "console"]),
                area("left", &["hierarchy", "agents"]),
            ),
            split(
                H,
                0.76,
                area(
                    "view",
                    &["material", "vfx", "scene", "game", "animation", "profiler"],
                ),
                area("right", &["inspector"]),
            ),
        ),
        "animation" => split(
            H,
            0.17,
            split(
                V,
                0.5,
                area("left", &["hierarchy"]),
                area("bottom", &["assets", "console", "agents"]),
            ),
            split(
                H,
                0.76,
                area(
                    "view",
                    &["animation", "scene", "game", "material", "vfx", "profiler"],
                ),
                area("right", &["inspector"]),
            ),
        ),
        "profiling" => split(
            V,
            0.45,
            split(
                H,
                0.8,
                area("view", &["scene", "game", "material", "vfx", "animation"]),
                area("right", &["inspector", "hierarchy"]),
            ),
            area("bottom", &["profiler", "console", "assets", "agents"]),
        ),
        "laptop" => split(
            H,
            0.19,
            area("left", &["hierarchy", "assets"]),
            split(
                H,
                0.74,
                area("view", DOCUMENTS),
                area("right", &["inspector", "console", "agents"]),
            ),
        ),
        _ => return None,
    })
}
/// View payload stored as a reflected component in the navigation op document.
#[derive(Schema)]
#[ara(component)]
#[allow(dead_code)] // Schema declaration; navigation is stored as a canonical JSON value.
struct EditorView {
    /// Versioned navigation state, opaque to gameplay schemas.
    value: Value,
}
pub(crate) struct Navigation {
    store: Store,
    doc: AssetId,
    record: RecordId,
    pub state: EditorState,
}
impl Navigation {
    pub fn history(&self) -> Value {
        json!(self.store.history())
    }
    pub fn new(root: &Path, mut state: EditorState) -> anyhow::Result<Self> {
        let path = root.join(".ara/editor-layout.json");
        if path.exists() {
            let mut layout =
                DockLayout::new(default_layout()).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            // A layout from an older editor that fails validation falls back to the default.
            if layout.restore(&std::fs::read_to_string(path)?).is_ok() {
                state.set_dock(&layout);
            }
        }
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        db.register::<EditorView>();
        let doc = AssetId::parse("sc_editor00").map_err(anyhow::Error::msg)?;
        let record = RecordId::parse("e_editor00").map_err(anyhow::Error::msg)?;
        let mut document = Document::new("scene", doc.clone());
        document.insert_unchecked(
            record.clone(),
            json!({"EditorView":{"value":state}})
                .as_object()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("navigation object"))?,
        );
        let mut ws = Workspace::default();
        ws.insert(".ara/editor-state.scene", document);
        Ok(Self {
            store: Store::new(db, ws),
            doc,
            record,
            state,
        })
    }
    pub fn commit(
        &mut self,
        candidate: EditorState,
        author: Author,
        why: &str,
        root: &Path,
    ) -> Result<Value, Vec<ara_diag::Diagnostic>> {
        let layout_changed =
            candidate.layout != self.state.layout || candidate.floating != self.state.floating;
        let value =
            serde_json::to_value(&candidate).map_err(|e| vec![crate::invalid(e.to_string())])?;
        let mut meta = TxnMeta::by(author);
        meta.why = Some(why.into());
        meta.label = Some(why.into());
        let edit = Edit::Set {
            doc: self.doc.clone(),
            id: self.record.clone(),
            path: "EditorView.value"
                .parse::<FieldPath>()
                .map_err(|e| vec![crate::invalid(e)])?,
            value: Some(value),
        };
        let commit = self.store.transact(meta, [edit])?;
        let result = json!({"state":candidate,"txn":commit.txn(),"revision":self.store.revision()});
        if layout_changed {
            persist_layout(root, &candidate).map_err(|e| {
                vec![ara_diag::Diagnostic::error(
                    ara_diag::codes::IO,
                    e.to_string(),
                )]
            })?;
        }
        self.state = candidate;
        Ok(result)
    }
}
/// Write the main dock tree and floating windows with the canonical writer.
pub(crate) fn persist_layout(root: &Path, state: &EditorState) -> anyhow::Result<()> {
    write_layout(&root.join(".ara/editor-layout.json"), state)
}
/// Save a named layout (Window › Layouts › Save Layout As…).
pub(crate) fn write_layout(path: &Path, state: &EditorState) -> anyhow::Result<()> {
    let text = state
        .dock()
        .and_then(|d| d.serialize())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
/// Project-local saved layouts, by file stem.
pub(crate) fn saved_layouts(root: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(root.join(".ara/layouts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            (path.extension()? == "json").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    names.sort();
    names
}
