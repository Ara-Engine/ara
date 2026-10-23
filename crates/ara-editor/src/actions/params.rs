use super::*;
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
/// Parameters for a human review decision on a deferred agent mutation.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorPending {
    /// Live protocol session.
    #[expect(dead_code, reason = "Host validates sessions before dispatch")]
    pub session: String,
    /// Ticket returned by a queued mutation.
    pub ticket: String,
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
/// Parameters for an explicit orbit camera pose.
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
    /// Orbit centre, in metres.
    #[ara(unit=Meters)]
    pub target: [f32; 3],
    /// Azimuth, in radians.
    pub yaw: f32,
    /// Elevation, in radians.
    pub pitch: f32,
    /// Positive orbit distance, in metres.
    #[ara(unit=Meters)]
    pub distance: f32,
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
/// Parameters for the agent application boundary.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EditorPolicy {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// Desired D20 policy.
    pub policy: ApplyPolicy,
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
