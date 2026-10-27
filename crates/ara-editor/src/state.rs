//! Navigation state is an op document with a history independent of scene undo.
mod layout_file;
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_render_fidelity::presets::{QualityChoice, QualityPreset};
use ara_schema::{FieldPath, Schema, SchemaDb};
use ara_ui::docking::{Axis, DockDrop, DockLayout, DockNode, Floating};
use glam::Vec3;
pub(crate) use layout_file::{MIGRATION_NOTICE, restore as restore_layout};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[cfg(test)]
#[path = "camera_tests.rs"]
mod camera_tests;

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
    /// Move, rotate or scale with the handle grabbed in the combined gizmo.
    Universal,
}
/// Projection used by the editor's scene camera.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraProjection {
    /// Depth changes apparent size, as through a perspective lens.
    #[default]
    Perspective,
    /// Parallel sight lines retain world-space dimensions at every depth.
    Orthographic,
}
/// Scene camera; this is editor state, never gameplay state.
/// Fields missing from older saved state take their defaults.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Camera {
    /// Orbit centre in metres.
    pub target: Vec3,
    /// Azimuth in radians.
    pub yaw: f32,
    /// Elevation in radians, bounded away from the poles.
    pub pitch: f32,
    /// Orbit radius in metres.
    pub distance: f32,
    /// Vertical field of view in degrees, within [`Camera::FOV_RANGE`]; set it with
    /// [`Camera::set_fov`]. Matches a scene `Camera.fov` for parity shots.
    pub fov_y_degrees: f32,
    /// Perspective or parallel scene projection.
    pub projection: CameraProjection,
    /// Visible vertical span in metres when using orthographic projection.
    pub orthographic_height: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::new(0., 0.5, 0.),
            yaw: 0.6,
            pitch: 0.4,
            distance: 10.,
            fov_y_degrees: Self::DEFAULT_FOV,
            projection: CameraProjection::Perspective,
            orthographic_height: 10.,
        }
    }
}
impl Camera {
    /// The Scene view's vertical field of view until one is chosen, in degrees.
    pub const DEFAULT_FOV: f32 = 50.;
    /// Vertical fields of view the Scene view accepts, in degrees.
    pub const FOV_RANGE: std::ops::RangeInclusive<f32> = 10.0..=120.0;
    /// Set the vertical field of view, clamped to [`Camera::FOV_RANGE`].
    pub fn set_fov(&mut self, degrees: f32) {
        self.fov_y_degrees = degrees.clamp(*Self::FOV_RANGE.start(), *Self::FOV_RANGE.end());
    }
    /// The field of view for labels: whole degrees or one decimal (`46°`, `47.5°`).
    pub fn fov_label(&self) -> String {
        let tenths = (self.fov_y_degrees * 10.).round() / 10.;
        if tenths.fract() == 0. {
            format!("{tenths:.0}°")
        } else {
            format!("{tenths:.1}°")
        }
    }
    /// Look from `position` along `forward` (a scene camera's pose), keeping the
    /// orbit distance so the pivot lies straight ahead. The orbit camera has no
    /// roll, and views within ~2° of vertical keep the pitch limit.
    pub fn look_from(&mut self, position: Vec3, forward: Vec3) {
        let back = -forward.try_normalize().unwrap_or(Vec3::NEG_Z);
        self.pitch = back.y.clamp(-1., 1.).asin().clamp(-1.54, 1.54);
        self.yaw = back.x.atan2(back.z);
        self.target = position - (self.position() - self.target);
    }
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
        if self.projection == CameraProjection::Orthographic {
            return ara_render::scene::CameraSpec::OrthographicLookAt {
                position: self.position(),
                target: self.target,
                vertical_size: self.orthographic_height,
            };
        }
        ara_render::scene::CameraSpec::LookAt {
            position: self.position(),
            target: self.target,
            vertical_fov: self.fov_y_degrees.to_radians(),
        }
    }
    /// Pan in the camera's image plane, in logical pixels.
    pub fn pan(&mut self, delta: [f32; 2]) {
        if !delta.iter().all(|value| value.is_finite()) {
            return;
        }
        let Some((right, up, _)) = self.basis() else {
            return;
        };
        let scale = if self.projection == CameraProjection::Orthographic {
            self.orthographic_height * 0.0015
        } else {
            self.distance * 0.0015
        };
        self.target += (-right * delta[0] + up * delta[1]) * scale;
    }
    /// Rotate without permitting upside-down orbit singularities.
    pub fn orbit(&mut self, delta: [f32; 2]) {
        if !delta.iter().all(|value| value.is_finite()) {
            return;
        }
        self.yaw -= delta[0] * 0.005;
        self.pitch = (self.pitch + delta[1] * 0.005).clamp(-1.54, 1.54);
    }
    /// Turn a fly camera around its own position, leaving orbit's pivot semantics intact.
    pub fn look(&mut self, delta: [f32; 2]) {
        let position = self.position();
        self.orbit(delta);
        let offset = self.position() - self.target;
        self.target = position - offset;
    }
    /// Exponential dolly, bounded to useful editor scales.
    pub fn zoom(&mut self, amount: f32) {
        if !amount.is_finite() {
            return;
        }
        if self.projection == CameraProjection::Orthographic {
            self.orthographic_height =
                (self.orthographic_height * (-amount * 0.12).exp()).clamp(0.01, 1_000_000.);
        } else {
            self.distance = (self.distance * (-amount * 0.12).exp()).clamp(0.05, 100_000.);
        }
    }
    /// Fly in view-local coordinates at a speed supplied by the input controller.
    pub fn fly(&mut self, direction: Vec3, metres: f32) {
        if !direction.is_finite() || !metres.is_finite() {
            return;
        }
        if let Some((right, up, forward)) = self.basis() {
            self.target +=
                (right * direction.x + up * direction.y - forward * direction.z) * metres;
        }
    }
    /// Switch projection while preserving the visible height at the orbit pivot.
    pub fn set_projection(&mut self, projection: CameraProjection) {
        if projection == self.projection {
            return;
        }
        let tangent = (self.fov_y_degrees.to_radians() * 0.5).tan();
        if projection == CameraProjection::Orthographic {
            self.orthographic_height = (2. * self.distance * tangent).clamp(0.01, 1_000_000.);
        } else {
            self.distance = (self.orthographic_height / (2. * tangent)).clamp(0.05, 100_000.);
        }
        self.projection = projection;
    }
    /// World-space image-plane axes and viewing direction, matching CameraSpec.
    pub fn basis(&self) -> Option<(Vec3, Vec3, Vec3)> {
        let forward = (self.target - self.position()).try_normalize()?;
        let seed = if forward.dot(Vec3::Y).abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let right = forward.cross(seed).try_normalize()?;
        Some((right, right.cross(forward), forward))
    }
    /// A finite pointer ray from NDC coordinates (right/up), using actual viewport pixels.
    /// Orthographic origins follow the near plane; perspective origins stay at the eye.
    pub fn ray(&self, size: [u32; 2], ndc: [f32; 2]) -> Option<(Vec3, Vec3)> {
        if !ndc.iter().all(|value| value.is_finite()) {
            return None;
        }
        let camera = self.spec().resolve(size).ok()?;
        if self.projection == CameraProjection::Orthographic {
            let (right, up, _) = self.basis()?;
            let origin = camera.position
                + right
                    * (ndc[0] * self.orthographic_height * size[0] as f32 / size[1] as f32 * 0.5)
                + up * (ndc[1] * self.orthographic_height * 0.5)
                + camera.forward * ara_render::scene::CameraSpec::ORTHOGRAPHIC_NEAR;
            return origin.is_finite().then_some((origin, camera.forward));
        }
        let inverse = (camera.projection * camera.view).inverse();
        let near = inverse * glam::Vec4::new(ndc[0], ndc[1], 1., 1.);
        if !near.is_finite() || near.w.abs() < 1e-8 {
            return None;
        }
        let near = near.truncate() / near.w;
        let ray = (camera.position, (near - camera.position).try_normalize()?);
        (ray.0.is_finite() && ray.1.is_finite()).then_some(ray)
    }
    /// Metres per logical pixel at a pivot, for constant-size transform handles.
    pub fn world_per_pixel(&self, pivot: Vec3, height: f32) -> Option<f32> {
        if !height.is_finite() || height <= 0. || !pivot.is_finite() {
            return None;
        }
        let value = if self.projection == CameraProjection::Orthographic {
            self.orthographic_height / height
        } else {
            2. * (pivot - self.position()).dot(self.basis()?.2).max(0.01)
                * (self.fov_y_degrees.to_radians() * 0.5).tan()
                / height
        };
        (value.is_finite() && value > 0.).then_some(value)
    }
    /// Whether this camera can produce finite matrices and a meaningful view.
    pub fn is_valid(&self) -> bool {
        self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.distance.is_finite()
            && self.distance > 0.
            && self.fov_y_degrees.is_finite()
            && Self::FOV_RANGE.contains(&self.fov_y_degrees)
            && self.orthographic_height.is_finite()
            && self.orthographic_height > 0.
            && self.spec().resolve([1, 1]).is_ok()
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
    /// Dock areas shown as just their tab strip.
    pub collapsed_areas: BTreeSet<String>,
    /// Use Ara Light instead of the default Ara Dark.
    pub light: bool,
    /// Disable decorative transitions while preserving all interaction feedback.
    pub reduce_motion: bool,
    /// Scene viewport quality override, the user preference `viewport.quality`:
    /// `None` uses the project's `rendering.quality`, `Some(Auto)` detects from the
    /// GPU, `Some(Preset(..))` forces Low/Medium/High/Epic. Serialized as text
    /// (absent, `auto`, `low` … `epic`).
    #[serde(with = "render_quality_text")]
    pub render_quality: Option<QualityChoice>,
    /// This GPU's automatic preset (`QualityPreset::detect`), filled when the
    /// window's device starts; runtime only, never persisted.
    #[serde(skip)]
    pub detected_quality: Option<QualityPreset>,
    /// Use the same-device fidelity view for Scene lighting: false only when the
    /// effective quality resolves to Low. Derived from `render_quality` (and the
    /// project's default) in `apply_settings`; never set it directly.
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
    /// Collapsed hierarchy addresses.
    pub collapsed: BTreeSet<String>,
    /// Large hierarchy groups the user expanded: groups of
    /// [`LARGE_GROUP`](crate::shell::LARGE_GROUP) or more children start collapsed.
    pub expanded: BTreeSet<String>,
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
    /// Each scene's last camera, restored when the scene is reopened.
    pub scene_cameras: BTreeMap<String, Camera>,
    /// Named camera views per scene, recorded as ordinary navigation ops.
    pub camera_bookmarks: BTreeMap<String, BTreeMap<String, Camera>>,
    /// Scenes the human opened, newest first (File > Open Recent).
    pub recent_scenes: Vec<String>,
    /// Selected source media path; separate from entity selection.
    pub asset_path: Option<String>,
    /// Folder shown by the asset browser, relative to the project root.
    pub asset_folder: String,
    /// Folder sidebar width in logical pixels, saved per project.
    pub asset_sidebar_width: f32,
    /// Explicitly expanded asset folders; all other folders start closed.
    pub asset_expanded_folders: BTreeSet<String>,
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
            collapsed_areas: DEFAULT_COLLAPSED.iter().map(|a| (*a).to_owned()).collect(),
            light: false,
            reduce_motion: false,
            render_quality: None,
            detected_quality: None,
            quality: true,
            camera: Camera::default(),
            tool: Tool::Translate,
            local: false,
            centre: false,
            snapping: false,
            move_snap: 0.5,
            rotate_snap: 15.,
            scale_snap: 0.1,
            collapsed: BTreeSet::new(),
            expanded: BTreeSet::new(),
            hidden: BTreeSet::new(),
            locked: BTreeSet::new(),
            searches: BTreeMap::new(),
            palette: false,
            presence: BTreeMap::new(),
            follow: None,
            scene_cameras: BTreeMap::new(),
            camera_bookmarks: BTreeMap::new(),
            recent_scenes: Vec::new(),
            asset_path: None,
            asset_folder: String::new(),
            asset_sidebar_width: 168.,
            asset_expanded_folders: BTreeSet::new(),
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
    /// Whether `other` renders a different Scene image: the document, camera, view
    /// mode, viewport-hidden entities or quality. Selection, gizmos and the grid are
    /// UI overlays and need no Scene frame.
    pub(crate) fn scene_changed(&self, other: &Self) -> bool {
        self.doc != other.doc
            || self.camera != other.camera
            || self.view_mode != other.view_mode
            || self.hidden != other.hidden
            || self.quality != other.quality
            || self.render_quality != other.render_quality
    }
    pub(crate) fn panels_changed(&self, other: &Self) -> bool {
        self.doc != other.doc
            || self.camera.projection != other.camera.projection
            || self.camera.fov_y_degrees != other.camera.fov_y_degrees
            || self.camera.orthographic_height != other.camera.orthographic_height
            || self.camera_bookmarks != other.camera_bookmarks
            || self.asset_path != other.asset_path
            || self.asset_folder != other.asset_folder
            || self.asset_sidebar_width != other.asset_sidebar_width
            || self.asset_expanded_folders != other.asset_expanded_folders
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
            || self.collapsed_areas != other.collapsed_areas
            || self.light != other.light
            || self.reduce_motion != other.reduce_motion
            || self.quality != other.quality
            || self.render_quality != other.render_quality
            || self.detected_quality != other.detected_quality
            || self.tool != other.tool
            || self.local != other.local
            || self.centre != other.centre
            || self.snapping != other.snapping
            || self.collapsed != other.collapsed
            || self.expanded != other.expanded
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
            self.render_quality = match quality.as_str() {
                "project" => None,
                other => other.parse().ok(),
            };
        }
        // The one place `quality` is derived: only Low is the raster view.
        self.quality = self.resolve_quality(self.render_choice()) != Some(QualityPreset::Low);

        if let Some(v) = self
            .settings
            .get("appearance.reduceMotion")
            .and_then(Value::as_bool)
        {
            self.reduce_motion = v;
        }
    }
    /// The project's default quality (`ara.project.json` `rendering.quality`).
    pub fn project_quality(&self) -> QualityChoice {
        self.settings
            .get("rendering.quality")
            .and_then(Value::as_str)
            .and_then(|q| q.parse().ok())
            .unwrap_or_default()
    }
    /// What the Scene view renders with: the override, else the project's default.
    pub fn render_choice(&self) -> QualityChoice {
        self.render_quality
            .unwrap_or_else(|| self.project_quality())
    }
    /// The preset `choice` runs at on this GPU (`None` while Auto is undetected).
    pub fn resolve_quality(&self, choice: QualityChoice) -> Option<QualityPreset> {
        match choice {
            QualityChoice::Preset(preset) => Some(preset),
            QualityChoice::Auto => self.detected_quality,
        }
    }
    /// A quality as people read it: `High`, `Auto · High` (detected) or `Auto`.
    pub fn quality_label(&self, choice: QualityChoice) -> String {
        match (choice, self.resolve_quality(choice)) {
            (QualityChoice::Preset(preset), _) => preset_title(preset).into(),
            (QualityChoice::Auto, Some(preset)) => format!("Auto · {}", preset_title(preset)),
            (QualityChoice::Auto, None) => "Auto".into(),
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
        Ok(
            DockLayout::with_floating(self.layout.clone(), self.floating.clone())?
                .with_collapsed(self.collapsed_areas.clone()),
        )
    }
    /// Store a dock model back into navigation state. Focus returns to the Scene
    /// when the focused panel is no longer open.
    pub fn set_dock(&mut self, dock: &DockLayout) {
        self.layout = dock.root().clone();
        self.floating = dock.floating().to_vec();
        self.collapsed_areas = dock.collapsed();
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
        "hierarchy" => "outline",
        "inspector" => "right",
        "assets" | "console" | "problems" | "agents" => "bottom",
        _ => "view",
    }
}
/// Panels that open in a floating window of this size when they are not in the
/// layout: tool pages, the Profiler and Settings. Docking one keeps it docked.
pub fn float_size(panel: &str) -> Option<[i32; 2]> {
    match panel {
        "settings" => Some([960, 640]),
        "material" | "vfx" | "animation" => Some([1200, 760]),
        "profiler" => Some([980, 620]),
        _ => None,
    }
}
/// Show a panel: activate it where it is (expanding a collapsed area), or open it:
/// in a floating window for [`float_size`] panels (at `rect`, desktop pixels; the
/// caller centres it on the editor), else in its home area.
pub fn show_panel(
    dock: &mut DockLayout,
    panel: &str,
    rect: Option<[i32; 4]>,
) -> ara_ui::Result<()> {
    if !dock.contains(panel) {
        match float_size(panel) {
            Some([w, h]) => {
                let id = (1..)
                    .map(|n| format!("window-{n}"))
                    .find(|id| dock.floating().iter().all(|f| &f.id != id))
                    .unwrap_or_default();
                // A closed panel floats from nowhere: open it, then tear it off.
                move_panel(dock, panel, "view")?;
                dock.float(panel, &id, rect.unwrap_or([160, 120, w, h]))?;
            }
            None => move_panel(dock, panel, home(panel))?,
        }
    }
    dock.expand_panel(panel);
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
        "left" | "outline" => (Axis::Horizontal, 0.18, true),
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
        return Err("ARA4042: dock destination missing".into());
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
/// Areas the default layout shows collapsed (none; the centre bottom dock starts open).
pub const DEFAULT_COLLAPSED: &[&str] = &[];
/// The default layout with its collapsed areas (new projects, Reset Layout).
pub fn default_dock() -> ara_ui::Result<DockLayout> {
    Ok(DockLayout::new(default_layout())?.collapsed_by_default(DEFAULT_COLLAPSED))
}
/// Built-in layouts. The default places full-height Hierarchy and Inspector beside
/// Scene/Game above a centre-only bottom dock. Tool pages and the Profiler open
/// as floating windows ([`float_size`]).
pub fn preset_layout(name: &str) -> Option<DockNode> {
    use Axis::{Horizontal as H, Vertical as V};
    const DOCUMENTS: &[&str] = &["scene", "game", "material", "vfx", "animation", "profiler"];
    Some(match name {
        "default" => split(
            H,
            0.18,
            area("outline", &["hierarchy"]),
            split(
                H,
                0.73,
                split(
                    V,
                    0.72,
                    area("view", &["scene", "game"]),
                    area("bottom", &["assets", "console", "problems", "agents"]),
                ),
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
                area("bottom", &["assets", "console", "problems", "agents"]),
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
                area("bottom", &["assets", "console", "problems"]),
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
                area("bottom", &["assets", "console", "problems", "agents"]),
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
            area(
                "bottom",
                &["profiler", "console", "problems", "assets", "agents"],
            ),
        ),
        "laptop" => split(
            H,
            0.19,
            area("left", &["hierarchy", "assets"]),
            split(
                H,
                0.74,
                area("view", DOCUMENTS),
                area("right", &["inspector", "console", "problems", "agents"]),
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
    /// Saves `.ara/editor-layout.json` off the UI thread (every tab switch, split drag
    /// or window move changes it; a synchronous write once took 203 ms of a click).
    layout: crate::persist::BackgroundWriter<String>,
    /// The startup migration notice is emitted once, by a service call or tick.
    layout_migrated: bool,
}
impl Navigation {
    /// Consume the one-time saved-layout migration notice.
    pub fn take_layout_notice(&mut self) -> bool {
        std::mem::take(&mut self.layout_migrated)
    }
    /// Layout writes that failed since the last call.
    pub fn take_failures(&self) -> Vec<String> {
        self.layout.take_failures()
    }
    pub fn history(&self) -> Value {
        json!(self.store.history())
    }
    pub fn new(root: &Path, mut state: EditorState) -> anyhow::Result<Self> {
        let path = root.join(".ara/editor-layout.json");
        let mut layout_migrated = false;
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            if let Ok(saved) = ara_json::parse(&text) {
                if let Some(width) = saved["assets"]["sidebarWidth"]
                    .as_f64()
                    .filter(|w| w.is_finite())
                {
                    state.asset_sidebar_width = (width as f32).clamp(96., 480.);
                }
                if let Ok(folders) =
                    serde_json::from_value(saved["assets"]["expandedFolders"].clone())
                {
                    state.asset_expanded_folders = folders;
                }
            }
            if let Ok((layout, migrated)) = restore_layout(&text) {
                layout_migrated = migrated;
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
        let navigation = Self {
            store: Store::new(db, ws),
            doc,
            record,
            state,
            layout: crate::persist::BackgroundWriter::new("layout-writer", move |text: String| {
                crate::persist::write_text(&path, &text)
                    .map_err(|e| format!("The window layout was not saved: {e}"))
            }),
            layout_migrated,
        };
        if layout_migrated {
            navigation.layout.save(layout_text(&navigation.state)?);
        }
        Ok(navigation)
    }
    pub fn commit(
        &mut self,
        candidate: EditorState,
        author: Author,
        why: &str,
    ) -> Result<Value, Vec<ara_diag::Diagnostic>> {
        let layout_changed = candidate.layout != self.state.layout
            || candidate.asset_sidebar_width != self.state.asset_sidebar_width
            || candidate.asset_expanded_folders != self.state.asset_expanded_folders
            || candidate.floating != self.state.floating
            || candidate.collapsed_areas != self.state.collapsed_areas;
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
            self.layout.save(layout_text(&candidate).map_err(|e| {
                vec![ara_diag::Diagnostic::error(
                    ara_diag::codes::IO,
                    e.to_string(),
                )]
            })?);
        }
        self.state = candidate;
        Ok(result)
    }
}

/// The main dock tree and floating windows, as the canonical writer stores them.
fn layout_text(state: &EditorState) -> anyhow::Result<String> {
    layout_file::serialize(state)
}
/// Save a named layout (Window › Layouts › Save Layout As…).
pub(crate) fn write_layout(path: &Path, state: &EditorState) -> anyhow::Result<()> {
    Ok(crate::persist::write_text(path, &layout_text(state)?)?)
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

/// A preset's title: `Low`, `Medium`, `High`, `Epic`.
pub fn preset_title(preset: QualityPreset) -> &'static str {
    match preset {
        QualityPreset::Low => "Low",
        QualityPreset::Medium => "Medium",
        QualityPreset::High => "High",
        QualityPreset::Epic => "Epic",
    }
}

/// `EditorState::render_quality` as text: absent (the project's default), `auto`,
/// `low`, `medium`, `high` or `epic`.
mod render_quality_text {
    use ara_render_fidelity::presets::QualityChoice;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<QualityChoice>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(choice) => serializer.serialize_some(&choice.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<QualityChoice>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| text.parse().map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_is_derived_from_the_override_the_project_and_the_gpu() {
        let mut state = EditorState::default();
        state
            .settings
            .insert("viewport.quality".into(), json!("project"));
        state
            .settings
            .insert("rendering.quality".into(), json!("auto"));
        state.apply_settings();
        assert_eq!(state.render_quality, None);
        assert!(state.quality, "an undetected Auto is not Low");
        assert_eq!(state.quality_label(state.render_choice()), "Auto");
        // Auto on a GPU without ray tracing resolves to Low: the raster view.
        state.detected_quality = Some(QualityPreset::Low);
        state.apply_settings();
        assert!(!state.quality);
        assert_eq!(state.quality_label(state.render_choice()), "Auto · Low");
        state
            .settings
            .insert("viewport.quality".into(), json!("high"));
        state.apply_settings();
        assert_eq!(
            state.render_quality,
            Some(QualityChoice::Preset(QualityPreset::High))
        );
        assert!(state.quality);
        // Serialized as text; detection is runtime only.
        let value = serde_json::to_value(&state).expect("state serializes");
        assert_eq!(value["renderQuality"], "high");
        let back: EditorState = serde_json::from_value(value).expect("state deserializes");
        assert_eq!(back.render_quality, state.render_quality);
        assert_eq!(back.detected_quality, None);
    }
}
