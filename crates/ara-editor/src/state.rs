//! Navigation state is an op document with a history independent of scene undo.
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::{FieldPath, Schema, SchemaDb};
use ara_ui::docking::{Axis, DockLayout, DockNode};
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
    }
}
/// The registered default arrangement; IDs are stable protocol and persistence keys.
pub fn default_layout() -> DockNode {
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
    split(
        Axis::Vertical,
        0.75,
        split(
            Axis::Horizontal,
            0.18,
            area("left", &["hierarchy"]),
            split(
                Axis::Horizontal,
                0.73,
                area(
                    "view",
                    &["scene", "game", "material", "vfx", "animation", "profiler"],
                ),
                area("right", &["inspector"]),
            ),
        ),
        area("bottom", &["assets", "console", "agents"]),
    )
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
            layout
                .restore(&std::fs::read_to_string(path)?)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            state.layout = layout.root().clone();
        }
        fn extend(node: &mut DockNode) {
            match node {
                DockNode::Area { id, tabs, .. } if id == "view" => {
                    for panel in ["material", "vfx", "animation", "profiler"] {
                        if !tabs.iter().any(|t| t == panel) {
                            tabs.push(panel.into());
                        }
                    }
                }
                DockNode::Split { first, second, .. } => {
                    extend(first);
                    extend(second);
                }
                _ => {}
            }
        }
        extend(&mut state.layout);
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
        let layout_changed = candidate.layout != self.state.layout;
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
            persist_layout(root, &candidate.layout).map_err(|e| {
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
pub(crate) fn persist_layout(root: &Path, layout: &DockNode) -> anyhow::Result<()> {
    let path = root.join(".ara/editor-layout.json");
    let temp = path.with_extension("json.tmp");
    std::fs::write(
        &temp,
        ara_json::to_canonical_string(&serde_json::to_value(layout)?),
    )?;
    std::fs::rename(temp, path)?;
    Ok(())
}
