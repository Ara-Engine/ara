//! Editor control requests are additive to the player wire protocol.
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// A spectator view of a running world. Its state lives in the player's transient navigation op store.
#[derive(Clone, Debug, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "PlayEditorViewCamera")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewCamera {
    /// Eye position in world metres.
    #[ara(unit=Meters)]
    pub position: [f32; 3],
    /// A point the camera looks toward; it must differ from position.
    #[ara(unit=Meters)]
    pub target: [f32; 3],
    /// Vertical field of view, in degrees.
    #[ara(default=55,unit=Degrees,range(min=10,max=120))]
    pub fov_y_degrees: f32,
}
/// A per-player view switch. Null returns to that player's authored gameplay camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    /// Read the current typed navigation state without applying camera or speed changes.
    #[serde(default)]
    pub inspect: bool,
    /// Spectator camera; absent/null restores the gameplay camera.
    pub camera: Option<ViewCamera>,
    /// Start spectator mode from the current gameplay camera when no camera is supplied.
    #[serde(default)]
    pub capture_current: bool,
    /// Spectator movement speed in metres per second; absent retains the view's speed.
    #[serde(default)]
    pub navigation_speed: Option<f32>,
}
/// A possession change applied by the authority, never independently patched on mirrors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    /// Simulation input owner (1 local/listen host, 2+ client owners).
    pub owner: u64,
    /// Controllable entity address; null ejects the owner from its current actor.
    pub entity: Option<String>,
}
/// A possession acknowledgement after the boundary and ownership refresh completed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Controlled {
    /// Authority tick that applied the command's ops.
    pub applied_at_tick: u64,
    /// Input owner whose actor changed.
    pub owner: u64,
    /// Actor now controlled by the owner; null means ejected.
    pub entity: Option<String>,
    /// Actors relinquished by this owner in the same transaction.
    pub previous: Vec<String>,
}
