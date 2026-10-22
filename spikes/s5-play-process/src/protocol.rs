//! The `play.*` method set of the live link. Every method is a typed [`Method`]; the host calls
//! them on a player, except `player.hello` and `play.status`, which the player sends to the host.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::SceneId;
use crate::registry::Op;
use crate::rpc::Method;

macro_rules! method {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $params:ty => $out:ty) => {
        $(#[$doc])*
        pub struct $ty;
        impl Method for $ty {
            const NAME: &'static str = $name;
            type Params = $params;
            type Output = $out;
        }
    };
}

method!(
    /// Player → host, first message after connecting. The host checks the token and protocol.
    Hello, "player.hello", HelloParams => HelloResult
);
method!(
    /// Load a snapshot and start ticking. Answered after tick 1 (or at tick 0 if `startPaused`).
    Start, "play.start", StartParams => StartResult
);
method!(
    /// Read an entity's components now, between ticks.
    Inspect, "play.inspect", InspectParams => InspectResult
);
method!(
    /// Apply ops atomically at the next tick boundary; answered after that tick has run.
    Patch, "play.patch", PatchParams => PatchResult
);
method!(Pause, "play.pause", Empty => TickResult);
method!(Resume, "play.resume", Empty => TickResult);
method!(
    /// Run exactly `ticks` ticks now (paused players only), as fast as possible.
    Step, "play.step", StepParams => CaptureResult
);
method!(
    /// State hash of the running world.
    Capture, "play.capture", Empty => CaptureResult
);
method!(
    /// Ops that turn the starting snapshot into the current runtime state ("Keep changes").
    Diff, "play.diff", DiffParams => DiffResult
);
method!(
    /// Stop playing; the player answers and exits.
    Stop, "play.stop", Empty => TickResult
);
method!(
    /// Test hook: crash or hang the player on purpose. Never answered.
    Crash, "play.debug.crash", CrashParams => Empty
);
method!(
    /// Player → host heartbeat notification, every 250 ms of wall time.
    Status, "play.status", StatusParams => Empty
);

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Empty {}

/// Which part of a multiplayer session a player process plays.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Standalone,
    Server,
    Client,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HelloParams {
    pub token: String,
    pub protocol: String,
    pub pid: u32,
    pub name: String,
    pub role: Role,
    /// Server role only: the game-transport address clients connect to.
    pub net_addr: Option<String>,
    /// Time from `main` to sending hello.
    pub init_us: u64,
    /// Vulkan startup cost, when `--gpu-init` is set.
    pub gpu: Option<GpuTimings>,
}

/// Where a player's Vulkan startup time goes (`--gpu-init`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct GpuTimings {
    /// Loading the Vulkan loader DLL.
    pub loader_us: u64,
    /// `vkCreateInstance`, including ICD and implicit-layer loading.
    pub instance_us: u64,
    /// Physical-device selection plus `vkCreateDevice`.
    pub device_us: u64,
}

impl GpuTimings {
    pub fn total_us(&self) -> u64 {
        self.loader_us + self.instance_us + self.device_us
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    pub session: String,
}

/// Where the starting snapshot is.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "transport", rename_all = "camelCase")]
pub enum SnapshotRef {
    /// Canonical JSON scene document in a blob frame.
    WsJson { blob: u64 },
    /// Binary columnar snapshot in a blob frame.
    WsBinary { blob: u64 },
    /// Binary columnar snapshot in a named shared-memory section.
    Shm { section: String, len: u64 },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    pub snapshot: SnapshotRef,
    pub start_paused: bool,
    /// Client role only: the server's game-transport address.
    pub connect_to: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub tick: u64,
    pub entities: u64,
    /// Snapshot decode plus ECS spawn.
    pub decode_us: u64,
    pub first_tick_us: u64,
    /// Client role only: UDP hello/welcome with the server.
    pub net_handshake_us: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InspectParams {
    pub target: SceneId,
    pub components: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InspectResult {
    pub tick: u64,
    pub components: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PatchParams {
    pub ops: Vec<Op>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PatchResult {
    /// The tick boundary the ops were applied at; the world then ran to `tick`.
    pub applied_at_tick: u64,
    pub tick: u64,
    /// Undo ops for this patch, in undo order.
    pub inverse: Vec<Op>,
    /// For each `set` op, the value at its path after `tick` ran (`None` for other ops).
    pub read_back: Vec<Option<Value>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StepParams {
    pub ticks: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResult {
    pub tick: u64,
    pub hash: String,
    pub entities: u64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TickResult {
    pub tick: u64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiffParams {
    /// Entities to compare; `None` compares all of them.
    pub targets: Option<Vec<SceneId>>,
    /// Components to compare; `None` compares all of them.
    pub components: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub tick: u64,
    /// Ops against the starting snapshot; `prev` holds the snapshot value.
    pub ops: Vec<Op>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CrashKind {
    Panic,
    AccessViolation,
    Hang,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CrashParams {
    pub kind: CrashKind,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StatusParams {
    pub tick: u64,
    pub paused: bool,
    /// Server role only: clients that completed the game handshake.
    pub clients: u32,
    /// Worst tick start lateness against the fixed schedule since the last status.
    pub max_late_us: u64,
}

/// Formats a state hash for JSON (u64 does not survive JSON numbers).
pub fn hash_hex(hash: u64) -> String {
    format!("{hash:016x}")
}
