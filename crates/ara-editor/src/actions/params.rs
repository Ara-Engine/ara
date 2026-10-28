use super::*;
/// Freeze a complete profiler capture for inspection and comparison.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorProfileFreeze {
    /// Calling editor or agent session.
    #[expect(dead_code, reason = "Host validates sessions")]
    pub session: String,
    /// Store as the comparison baseline instead of the current capture.
    pub baseline: Option<bool>,
}
/// Shared node-canvas navigation. Content edits use graph/material commands.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorCanvas {
    /// Calling editor or agent session.
    #[expect(dead_code, reason = "Host validates sessions")]
    pub session: String,
    /// Logical canvas pan.
    pub pan: Option<[f32; 2]>,
    /// Zoom between 0.2 and 2.5.
    pub zoom: Option<f32>,
    /// Selected stable graph nodes.
    pub selection: Option<Vec<String>>,
    /// Open the search-to-add palette.
    pub add: Option<bool>,
}
/// Asset-browser and authoring-panel navigation, independent of scene selection.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorWorkspace {
    /// Calling editor or agent session.
    #[expect(dead_code, reason = "Host validates sessions")]
    pub session: String,
    /// Source media to inspect; an empty string clears the selection.
    pub asset: Option<String>,
    /// Folder to browse.
    pub folder: Option<String>,
    /// Folder sidebar width in logical pixels (96–480).
    pub sidebar_width: Option<f32>,
    /// Toggle the expansion of one project-relative folder.
    pub toggle_folder: Option<String>,
    /// Explicitly expand or collapse `toggleFolder`; absent toggles it.
    pub expand_folder: Option<bool>,
    /// Thumbnail grid or compact list.
    pub grid: Option<bool>,
    /// Resource graph or controller to open.
    pub resource: Option<String>,
    /// Module or controller-state address to inspect.
    pub item: Option<String>,
    /// Frame identity in the profiler timeline.
    pub frame: Option<u64>,
    /// Show the two-capture comparison.
    pub compare: Option<bool>,
    /// Show animation pose nodes and blend-space samples.
    pub blends: Option<bool>,
    /// Run the live VFX preview.
    pub playing: Option<bool>,
    /// Show a plane instead of a sphere in the material preview.
    pub plane: Option<bool>,
}
/// Follow an agent's separate selection/camera without letting it interrupt the human by default.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorFollow {
    /// Human reviewer session.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Agent session to follow; absent restores independent human navigation.
    pub agent: Option<String>,
}
/// Native host diagnostics use the same diagnostic bus as engine services.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorReport {
    /// Live protocol session.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Actionable editor error message.
    pub message: String,
    /// What failed: `action` (default), `thumbnail`, `settings`, `transport`, `render`
    /// or `missing` (a scene's asset file).
    pub kind: Option<String>,
    /// What it is about (`asset:@textures/a.png`, `entity:e_8b21`); default `system:editor`.
    pub subject: Option<String>,
}
/// Parameters of `editor.scene.open`.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorSceneOpen {
    /// Live protocol session.
    pub session: String,
    /// Project-relative scene path (`levels/hangar.scene`) or scene document ID.
    pub scene: String,
}
/// Parameters for selecting entities without consuming the content undo stack.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorSelect {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Active document, if changing scenes.
    pub doc: Option<String>,
    /// Entity addresses to select.
    pub entities: Vec<String>,
    /// Toggle these entities in the current selection.
    pub toggle: Option<bool>,
    /// Extend the selection instead of replacing it.
    pub additive: Option<bool>,
}
/// Parameters for focusing or opening a panel.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorFocus {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Stable panel identity.
    pub panel: String,
    /// Field path to reveal, when focusing the inspector.
    pub field: Option<String>,
}
/// Parameters of `editor.frame`.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorFrame {
    /// Live protocol session.
    pub session: String,
    /// Show the scene as it opens instead: its gameplay camera's view, else its
    /// content (terrain and ground excluded) from a three-quarter angle.
    pub opening: Option<bool>,
    /// Width divided by height of the Scene viewport; omitted uses its last known aspect.
    pub aspect: Option<f32>,
    /// Frame every renderable object while preserving the current selection.
    pub all: Option<bool>,
}
/// Parameters to read or change the Scene view camera. Omitted fields keep their
/// values; with none given the call only reads the camera.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorCamera {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// A scene entity with a `Camera` component: look from its world pose with its
    /// field of view (applied before the other fields).
    pub entity: Option<String>,
    /// Orbit centre, in metres.
    #[ara(unit=Meters)]
    pub target: Option<[f32; 3]>,
    /// Azimuth, in radians.
    pub yaw: Option<f32>,
    /// Elevation, in radians.
    pub pitch: Option<f32>,
    /// Positive orbit distance, in metres.
    #[ara(unit=Meters)]
    pub distance: Option<f32>,
    /// Vertical field of view, in degrees; clamped to 10–120.
    #[ara(unit=Degrees)]
    pub fov_y_degrees: Option<f32>,
    /// Perspective or parallel orthographic projection.
    pub projection: Option<crate::state::CameraProjection>,
    /// Positive visible vertical span for orthographic projection, in metres.
    #[ara(unit=Meters)]
    pub orthographic_height: Option<f32>,
}
/// Operation on named scene camera views.
#[derive(Clone, Copy, Schema, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CameraBookmarkAction {
    /// Read the chosen scene's named views without changing navigation.
    List,
    /// Save the caller's current view; an omitted name creates a unique View N.
    Save,
    /// Restore the named view for this caller, opening its scene if requested.
    Recall,
    /// Remove a named view without moving the camera.
    Delete,
}
/// Read or edit per-scene named camera views through navigation ops.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorCameraBookmark {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// List, save, recall or delete a named camera view.
    pub action: CameraBookmarkAction,
    /// View name; save generates View N when omitted.
    pub name: Option<String>,
    /// Scene path or identity; omitted uses this caller's current scene.
    pub scene: Option<String>,
}
/// Parameters for a theme switch.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorTheme {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// True chooses Ara Light; false chooses Ara Dark.
    pub light: bool,
}
/// Parameters for windowed multi-player Play (D19): one window per player.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorPlay {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Client windows besides the host's (default 1, at most 16).
    pub clients: Option<u32>,
    /// A renderer-free authority without a player window of its own.
    pub dedicated: Option<bool>,
    /// Simulated one-way latency for every client, in milliseconds.
    pub latency_ms: Option<u32>,
    /// Simulated jitter for every client, in milliseconds.
    pub jitter_ms: Option<u32>,
    /// Simulated packet loss for every client, in basis points (100 = 1%).
    pub loss: Option<u16>,
}
/// Parameters for a dock move, activation or splitter adjustment.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorLayout {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Panel to move or activate.
    pub panel: Option<String>,
    /// Destination area for a move.
    pub area: Option<String>,
    /// Tab insertion position.
    pub index: Option<u32>,
    /// Create this new area by splitting the destination.
    pub new_area: Option<String>,
    /// Split vertically instead of horizontally.
    pub vertical: Option<bool>,
    /// Insert the new split before its destination.
    pub before: Option<bool>,
    /// Path to a splitter: false means first, true means second.
    pub path: Option<Vec<bool>>,
    /// Floating window identity for a splitter adjustment; absent targets the main window.
    /// Requires `ratio`; panel moves and activation keep their existing area addressing.
    pub window: Option<String>,
    /// Split fraction, constrained to 0.1 through 0.9.
    pub ratio: Option<f32>,
}
/// Parameters for spatial handle settings.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorTool {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Translate, rotate or scale.
    pub tool: Tool,
    /// Axes follow the selected entity's rotation.
    pub local: Option<bool>,
    /// Use the common selection centre.
    pub centre: Option<bool>,
    /// Enable quantized gestures.
    pub snapping: Option<bool>,
    /// Translation increment in metres.
    pub move_snap: Option<f32>,
    /// Rotation increment in degrees.
    pub rotate_snap: Option<f32>,
    /// Scale increment.
    pub scale_snap: Option<f32>,
}
/// Parameters for a hierarchy's transient presentation toggles.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorEntityView {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Entity address.
    pub entity: String,
    /// Requested toggle state.
    pub enabled: bool,
}
/// Parameters for an indexed panel search.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorSearch {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Panel identity.
    pub panel: String,
    /// Search substring or filter expression.
    pub query: String,
}
/// Parameters for opening or closing the command palette.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorPalette {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Whether the palette is visible.
    pub open: bool,
}

/// Appearance and rendering preferences shared by native controls and protocol clients.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorPreferences {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Disable 140 ms decorative transitions.
    pub reduce_motion: Option<bool>,
    /// Legacy switch: true uses the project's default quality (Auto if that is Low),
    /// false the Low raster view. Prefer `renderQuality`.
    pub quality: Option<bool>,
    /// Scene viewport quality: `project` (the project's default), `auto`, `low`,
    /// `medium`, `high` or `epic`. A user preference, never saved in the project.
    pub render_quality: Option<String>,
}
/// Set or reset one setting; the edit is an undoable op attributed to the session.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorSettingsSet {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Setting key from `editor.settings`, e.g. `appearance.theme`.
    pub key: String,
    /// New value; omit with `reset: true` to restore the default.
    pub value: Option<Value>,
    /// Restore the default value.
    pub reset: Option<bool>,
}
/// Undo or redo the session's latest settings edit.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorSettingsUndo {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Redo instead of undo.
    pub redo: Option<bool>,
}
/// Settings window navigation: scope, category page and the modified-only filter.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorSettingsView {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// `user` (Preferences) or `project`.
    pub scope: Option<String>,
    /// Category page; empty shows all.
    pub category: Option<String>,
    /// Show only values that differ from their defaults.
    pub modified: Option<bool>,
}
/// Run a registry command (the same commands as menus, the palette and shortcuts).
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorCommand {
    /// Live protocol session, validated by the host.
    pub session: String,
    /// Command id from `editor.commands`, e.g. `create.cube`.
    pub id: String,
    /// Optional target: `{"entity": "e_…"}`, `{"asset": "path"}`, `{"panel": "inspector"}`, `{"field": "Transform.position"}`.
    pub target: Option<Value>,
}
/// Viewport presentation: debug view, game view, overlays and the maximized panel.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorView {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// `lit`, `baseColor`, `normals`, `depth`, `roughnessMetal`, `entityIds`, `shadowCascades`, `wireframe` or `litWireframe`.
    pub view_mode: Option<String>,
    /// Hide every editor visual in the viewport.
    pub game_view: Option<bool>,
    /// Hide the viewport toolbar and widgets.
    pub hide_overlays: Option<bool>,
    /// Show transform handles (false is the Select tool).
    pub gizmos: Option<bool>,
    /// Show the orientation widget.
    pub orientation: Option<bool>,
    /// Panel to maximize; an empty string restores the layout.
    pub maximized: Option<String>,
    /// Play state shown by the toolbar: `playing`, `paused` or `stopped`.
    pub play: Option<String>,
}
/// A named layout: a built-in (`default`, `levelDesign`, `materials`, `animation`,
/// `profiling`, `laptop`) or one saved under `.ara/layouts`.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorLayoutName {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Layout name.
    pub name: String,
}
/// Show, close or move one panel.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorLayoutPanel {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Panel id, e.g. `inspector`.
    pub panel: String,
    /// Destination for moves: `left`, `right`, `bottom`, `view` or `float`.
    pub slot: Option<String>,
    /// Desktop rectangle (x, y, width, height) for a floating window.
    pub rect: Option<[f32; 4]>,
}
/// Collapse or expand a dock area.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorLayoutCollapse {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Area id, e.g. `bottom`.
    pub area: String,
    /// Show just the area's tab strip (`true`), or the whole area.
    pub collapsed: bool,
}
/// Record a floating window's position after the OS moved or resized it.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorWindow {
    /// Live protocol session, validated by the host.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Floating window id.
    pub id: String,
    /// Desktop rectangle: x, y, width, height.
    pub rect: [f32; 4],
}
