//! The play-mode wire contract between the edit-world owner (`ara` daemon or editor)
//! and `ara-player` processes (S5):
//!
//! - Players connect back to the owner over localhost WebSocket and speak JSON-RPC 2.0
//!   ([`link::Link`]); the first message is [`HELLO`] with the session token.
//! - The scene arrives as a binary [`PlayScene`] (CBOR) in a named shared-memory
//!   section ([`shm`]) or, where sections are unavailable, a binary frame.
//! - Methods are the `play.*` names below; results and notifications are the typed
//!   structs in this crate. Live edits use authoring paths, never raw ECS writes.
//! - A crashing player writes one [`CRASH_PREFIX`] line ([`CrashReport`]) to stderr.
#![warn(missing_docs)]

mod game_ui;
mod imports;
pub mod link;
pub mod shm;
pub use game_ui::{GameUi, UiBinding, UiMusic, UiSound};
pub use imports::{ImportState, Stall};

use ara_doc::{AssetId, Document, Record, RecordId, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Wire protocol identifier; bumped only for incompatible changes.
pub const PROTOCOL: &str = "ara-play/1";
/// Player → owner request: `{token, protocol, pid, name, window, initMs}` → `{}`.
pub const HELLO: &str = "player.hello";
/// Player → owner notification every 250 ms; also the liveness signal ([`Status`]).
pub const STATUS: &str = "play.status";
/// Player IO-thread liveness notification, independent of simulation/status work.
pub const HEARTBEAT: &str = "play.heartbeat";
/// Player → owner notification before known-long work (GPU device creation, world
/// build): the owner's hang watchdog allows silence until the deadline ([`Busy`]).
pub const BUSY: &str = "play.busy";
/// Player → owner notification: a C# reload report, a module diagnostic, or a
/// restart request after engine bindings changed (L4) ([`Event`]).
pub const EVENT: &str = "play.event";
/// Prefix of the single structured crash line a player writes to stderr.
pub const CRASH_PREFIX: &str = "ARA_PLAYER_CRASH ";
/// Environment variable carrying the per-session token (never the command line).
pub const TOKEN_ENV: &str = "ARA_PLAY_TOKEN";

/// Owner → player methods (requests with ids).
pub mod method {
    /// Start the assigned world from a snapshot ([`crate::Start`]).
    pub const START: &str = "play.start";
    /// Stop and exit; the player is single-use.
    pub const STOP: &str = "play.stop";
    /// Freeze host-time ticking.
    pub const PAUSE: &str = "play.pause";
    /// Resume host-time ticking.
    pub const RESUME: &str = "play.resume";
    /// Run exact ticks as fast as possible ([`crate::Step`]).
    pub const STEP: &str = "play.step";
    /// Run until a watched value satisfies a predicate ([`crate::Until`]).
    pub const UNTIL: &str = "play.until";
    /// Host-time speed multiplier.
    pub const TIMESCALE: &str = "play.timescale";
    /// Queue action injections for the next tick ([`crate::Inputs`]).
    pub const INPUT: &str = "play.input";
    /// Read components of one entity ([`crate::Inspect`]).
    pub const INSPECT: &str = "play.inspect";
    /// Read `entity.Component.field` values ([`crate::Read`]).
    pub const READ: &str = "play.read";
    /// Live edit applied at the next tick, acked after it ([`crate::Patch`]).
    pub const PATCH: &str = "play.patch";
    /// State hash and optionally a labelled image ([`crate::CaptureParams`]).
    pub const CAPTURE: &str = "play.capture";
    /// Replace watched paths sampled by step/until ([`crate::WatchParams`]).
    pub const WATCH: &str = "play.watch";
    /// Begin recording a replay.
    pub const RECORD: &str = "play.record";
    /// Finish recording and write the replay to a file ([`crate::SaveReplay`]).
    pub const SAVE_REPLAY: &str = "play.recording";
    /// Verify a replay file tick by tick ([`crate::SaveReplay`]).
    pub const REPLAY: &str = "play.replay";
    /// Field-level differences against the start documents, for "Keep changes".
    pub const DIFF: &str = "play.diff";
    /// Publish new asset generations, applied at the next tick boundary.
    pub const ASSETS: &str = "play.assets";
    /// Read-only import status notification ([`crate::ImportState`]); may precede Start.
    pub const IMPORTS: &str = "play.imports";
    /// Internal fault-test request: stall simulation while transport stays alive ([`crate::Stall`]).
    pub const STALL: &str = "play.debug.stall";
    /// Test builds: force a panic, access violation or hang ([`crate::Crash`]).
    pub const CRASH: &str = "play.debug.crash";
    /// Run a C# `[Command]` between ticks ([`crate::Command`]); returns its effects.
    pub const COMMAND: &str = "play.command";
    /// The C# command catalog (ara-cmd `Info` shape, `scope: "play"`).
    pub const COMMANDS: &str = "play.commands";
    /// Claim C# source paths as an agent edit before the owner writes them.
    pub const COMPILE: &str = "play.compile";
    /// A networked authority issues one client's join credential ([`crate::Join`] →
    /// [`crate::Joined`]). Private launch data: owners pass it to that client only.
    pub const JOIN: &str = "play.join";
    /// A portable snapshot of the current tick (hex CBOR), to resume elsewhere.
    pub const SNAPSHOT: &str = "play.snapshot";
    /// Render the world's sound offline to a WAV plus analysis ([`crate::CaptureAudio`]).
    pub const CAPTURE_AUDIO: &str = "play.captureAudio";
}

/// Process role, assigned when Play begins (same vocabulary as `ara-net` launches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Single authority world with local input (no networking).
    Local,
    /// Player-hosted authority (Listen).
    Host,
    /// Renderer-free authority.
    Dedicated,
    /// Remote player.
    Client,
}

/// `player.hello` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// Session token from [`TOKEN_ENV`].
    pub token: String,
    /// Must equal [`PROTOCOL`].
    pub protocol: String,
    /// OS process ID.
    pub pid: u32,
    /// Pool slot name.
    pub name: String,
    /// Whether this player owns a window.
    pub window: bool,
    /// Process start → hello, in milliseconds (init cost the pool hides).
    pub init_ms: f64,
}

/// Where the snapshot bytes are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "camelCase")]
pub enum SnapshotRef {
    /// A named read-only shared-memory section ([`shm`]).
    Shm {
        /// Section name.
        section: String,
        /// Payload length in bytes.
        len: u64,
    },
    /// Inline bytes (remote players, platforms without sections).
    Inline {
        /// The encoded [`PlayScene`].
        bytes: Vec<u8>,
    },
}

/// `play.start` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    /// The scene snapshot.
    pub snapshot: SnapshotRef,
    /// Process role.
    pub role: Role,
    /// Networked roles (host, dedicated, client): players and join data.
    #[serde(default)]
    pub net: Option<NetStart>,
    /// Stay paused after tick 0 (otherwise run at the fixed rate).
    pub start_paused: bool,
    /// Portable snapshot (hex CBOR, from `play.snapshot`) to restore after building:
    /// restarts keep the game state (C# L4 reloads).
    #[serde(default)]
    pub resume: Option<String>,
}

/// `play.command` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    /// Command name (`game.UseMedkit`).
    pub name: String,
    /// Params; entity params may use document addresses (`e_fox`).
    #[serde(default)]
    pub params: Value,
}

/// `play.captureAudio` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureAudio {
    /// WAV file to write (`<name>.audio.json` beside it holds the analysis).
    pub path: String,
    /// Seconds to render, in (0, 60].
    pub seconds: f64,
}

/// `play.compile` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compile {
    /// Absolute paths of C# sources an agent is about to write.
    pub paths: Vec<String>,
}

/// [`EVENT`] payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Event {
    /// A C# reload was applied (or failed) at a tick boundary.
    Reload {
        /// Tick boundary it applied at.
        tick: u64,
        /// `RELOAD_*` level (1–4, 100 = error).
        level: u32,
        /// Module slot.
        module: u32,
        /// 0 human, 1 agent.
        origin: u32,
        /// Modules swapped together.
        cascade: u32,
        /// Compile errors (the last good code keeps running).
        errors: u32,
    },
    /// A C# host or module diagnostic.
    Diagnostic {
        /// The diagnostic.
        diagnostic: Value,
    },
    /// Engine bindings changed (L4): restart the player from a snapshot.
    Restart,
    /// C# module types (after load and reloads) as project `*.types.json` documents
    /// by absolute path; the owner keeps the files current so every host validates
    /// C# components without starting .NET.
    Types {
        /// Path → types document.
        files: BTreeMap<String, Value>,
    },
}

/// C# modules a scene runs ([`PlayScene::scripts`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptScene {
    /// Development JIT by default; exports select a linked NativeAOT image.
    #[serde(default)]
    pub host: ScriptHost,
    /// `Ara.Host` JIT build output directory.
    pub host_dir: String,
    /// `Ara.CompileServer.dll` (hot reload); absent loads prebuilt assemblies.
    pub compile_server: Option<String>,
    /// Absolute module folders (each with `ara.module.toml`).
    pub modules: Vec<String>,
}

/// Script execution policy carried by a scene snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum ScriptHost {
    /// Author-machine compilation and hot reload for editor/Play.
    #[default]
    Development,
    /// Precompiled gameplay modules; no development loader or source folders.
    NativeAot {
        /// Manifest path relative to the exported player folder.
        manifest: String,
    },
}

impl ScriptScene {
    /// A portable exported script host without author paths or source modules.
    pub fn shipping(manifest: impl Into<String>) -> Self {
        Self {
            host: ScriptHost::NativeAot {
                manifest: manifest.into(),
            },
            host_dir: String::new(),
            compile_server: None,
            modules: Vec::new(),
        }
    }

    /// Resolves shipping metadata against the standalone export folder.
    pub fn resolve_shipping(&mut self, root: &std::path::Path) {
        if let ScriptHost::NativeAot { manifest } = &mut self.host {
            *manifest = root.join(&*manifest).to_string_lossy().into_owned();
        }
    }
}

/// `play.start` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    /// Completed tick (0 when started paused, else 1).
    pub tick: u64,
    /// Entities in the world.
    pub entities: usize,
    /// Snapshot decode + world build, in milliseconds.
    pub build_ms: f64,
    /// First tick, in milliseconds (0 when started paused).
    pub first_tick_ms: f64,
    /// Clients: netcode connect, schema check and timeline sync with the authority,
    /// in milliseconds.
    pub net_handshake_ms: Option<f64>,
}

/// Simulated link conditions for one client (`ara_net::Conditions`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkConditions {
    /// Added one-way latency, in milliseconds.
    pub latency_ms: u32,
    /// Uniform plus/minus one-way jitter, in milliseconds.
    pub jitter_ms: u32,
    /// Packet loss probability, in basis points (0..10000).
    pub loss: u16,
    /// Packet duplication probability, in basis points.
    pub duplication: u16,
    /// Extra-delay (reordering) probability, in basis points.
    pub reordering: u16,
    /// Directional bandwidth in bytes per second; 0 disables shaping.
    pub bandwidth: u32,
}

/// Networked play settings of [`Start`] (D19: Host+N, Dedicated+N).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetStart {
    /// Players in the session, the listen host's included (avatars up to this).
    pub players: u64,
    /// Authorities: one entry per client (players 2, 3, ...), conditions if simulated.
    #[serde(default)]
    pub clients: Vec<Option<LinkConditions>>,
    /// Clients: the join issued by the authority ([`method::JOIN`]).
    #[serde(default)]
    pub join: Option<Joined>,
}

/// `play.join` params: which client's credential to issue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Join {
    /// Client index (player `index + 2`).
    pub index: usize,
}

/// `play.join` result. The token is private: never log or persist it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Joined {
    /// The client's player.
    pub player: u64,
    /// Where the client connects (its link proxy on the authority).
    pub endpoint: String,
    /// Encrypted netcode connect token, hex.
    pub token: String,
}

/// One action injection, relative to the next tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// Ticks after the next boundary (0 = next tick).
    #[serde(default)]
    pub after_ticks: u32,
    /// Input owner.
    #[serde(default = "one")]
    pub player: u64,
    /// Declared action (`Move`, `Look`, `Jump`, `Sprint`).
    pub action: String,
    /// `true/false` for buttons, a number for axes, `[x, y]` for 2D axes.
    pub value: Value,
    /// Hold duration in ticks.
    #[serde(default = "one_u32")]
    pub hold_ticks: u32,
}
fn one() -> u64 {
    1
}
fn one_u32() -> u32 {
    1
}

/// `play.step` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// Ticks to run.
    pub ticks: u32,
    /// Injections relative to the first stepped tick.
    #[serde(default)]
    pub inputs: Vec<Input>,
}

/// Comparison for `play.until` and test assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Compare {
    /// Equal (numbers at f32 precision).
    Eq,
    /// Not equal.
    Ne,
    /// Greater than (numbers).
    Gt,
    /// Greater or equal.
    Ge,
    /// Less than.
    Lt,
    /// Less or equal.
    Le,
}

impl Compare {
    /// Applies the comparison to two JSON values (numbers compare as f64, other
    /// values only for (in)equality).
    pub fn test(self, actual: &Value, expected: &Value) -> bool {
        match (actual.as_f64(), expected.as_f64()) {
            (Some(a), Some(b)) => {
                let (fa, fb) = (a as f32, b as f32);
                match self {
                    Self::Eq => fa == fb,
                    Self::Ne => fa != fb,
                    Self::Gt => a > b,
                    Self::Ge => a >= b,
                    Self::Lt => a < b,
                    Self::Le => a <= b,
                }
            }
            _ => match self {
                Self::Eq => actual == expected,
                Self::Ne => actual != expected,
                _ => false,
            },
        }
    }
}

/// `play.until` params: tick until `path` compares true, at most `max_ticks`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Until {
    /// `entity.Component.field` path (`e_fox.Transform.position.z`).
    pub path: String,
    /// Comparison.
    pub compare: Compare,
    /// Right-hand value.
    pub value: Value,
    /// Tick budget.
    pub max_ticks: u32,
    /// Injections relative to the first tick.
    #[serde(default)]
    pub inputs: Vec<Input>,
}

/// One watched value after a tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// Tick after which it was sampled.
    pub tick: u64,
    /// Watched path.
    pub path: String,
    /// Value, absent for a missing entity/component.
    pub value: Option<Value>,
}

/// `play.step` / `play.until` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stepped {
    /// Ticks executed by this request.
    pub ticks: u32,
    /// Final tick.
    pub tick: u64,
    /// Whether the until predicate held.
    pub reached: bool,
    /// State hash after the last tick (hex).
    pub hash: String,
    /// Watch samples in tick order.
    pub samples: Vec<Sample>,
}

/// `play.input` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inputs {
    /// Injections, relative to the next tick.
    pub inputs: Vec<Input>,
}

/// `play.inspect` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspect {
    /// Entity address.
    pub entity: String,
    /// Components to read; all registered components when empty.
    #[serde(default)]
    pub components: Vec<String>,
}

/// `play.read` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Read {
    /// `entity.Component.field` paths.
    pub paths: Vec<String>,
}

/// One live edit: set (or clear) an authored value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Set {
    /// Entity address in the scene.
    pub entity: String,
    /// `Component.field` path.
    pub path: String,
    /// New value; absent restores the default.
    pub value: Option<Value>,
}

/// `play.patch` params: applied atomically at the next tick boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    /// Edits in order.
    pub sets: Vec<Set>,
}

/// `play.patch` result, sent after the tick that applied it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patched {
    /// Tick whose boundary applied the edits.
    pub applied_at_tick: u64,
    /// The values read back after that tick, one per set.
    pub read_back: Vec<Option<Value>>,
}

/// `play.capture` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureParams {
    /// Camera entity ID, document address or exact scene name; absent uses the active camera.
    #[serde(default)]
    pub camera: Option<String>,
    /// Per-capture rendering preset; absent uses the project's rendering policy.
    #[serde(default)]
    pub preset: Option<QualitySetting>,
    /// PNG path to write (plus `.marks.png`, `.legend.json`, `.json`); hash only when absent.
    pub path: Option<String>,
    /// Image width.
    #[serde(default = "width")]
    pub width: u32,
    /// Image height.
    #[serde(default = "height")]
    pub height: u32,
    /// Maximum numbered marks (0 disables marks).
    #[serde(default = "marks")]
    pub marks: usize,
    /// Render what windowed frames draw (terrain chunks as meshes, no landscape or
    /// VFX composite) instead of the full capture.
    #[serde(default)]
    pub presented: bool,
}
fn width() -> u32 {
    1280
}
fn height() -> u32 {
    720
}
fn marks() -> usize {
    40
}

/// One numbered mark in a capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    /// Number drawn on the image.
    pub mark: usize,
    /// `Name (e_address)` label.
    pub label: String,
    /// Visible-pixel box `[x, y, w, h]`.
    pub bbox: [u32; 4],
}

/// `play.capture` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Captured {
    /// Tick captured.
    pub tick: u64,
    /// State hash (hex).
    pub hash: String,
    /// Entities in the world.
    pub entities: usize,
    /// Written image path, if any.
    pub image: Option<String>,
    /// Numbered marks.
    pub marks: Vec<Mark>,
}

/// `play.watch` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchParams {
    /// `entity.Component.field` paths.
    pub paths: Vec<String>,
    /// Sampling interval in ticks.
    #[serde(default = "one_u32")]
    pub every: u32,
}

/// `play.recording` / `play.replay` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReplay {
    /// Replay file path (binary CBOR).
    pub path: String,
}

/// `play.debug.crash` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crash {
    /// `panic`, `access` or `hang`.
    pub kind: String,
}

/// `play.status` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Completed tick.
    pub tick: u64,
    /// Whether host time is frozen.
    pub paused: bool,
    /// Worst lateness of a tick start since the last status, in microseconds.
    pub max_late_us: u64,
    /// Presented frames per second (windowed players).
    pub fps: f64,
    /// p95 frame CPU time in milliseconds since the last status (windowed players).
    pub frame_ms_p95: f64,
    /// A subsystem fault that paused the world.
    pub fault: Option<String>,
    /// A live audio device is open and fed (windowed players with sound).
    #[serde(default)]
    pub audio: bool,
    /// Networked roles: authority traffic or client join/prediction numbers.
    #[serde(default)]
    pub net: Option<Value>,
    /// Resident cells of the streamed world (empty without one).
    #[serde(default)]
    pub cells: Vec<String>,
}

/// `play.busy` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Busy {
    /// What the player is doing (`gpu`, `build`).
    pub what: String,
    /// Longest expected silence in milliseconds; still a hang past it.
    pub max_ms: u64,
}

/// The single machine-readable crash line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashReport {
    /// `panic` or `exception`.
    pub kind: String,
    /// Panic message or exception name.
    pub message: String,
    /// `file:line:column` of a panic, or the faulting address of an exception.
    pub location: String,
    /// Last completed tick.
    pub tick: u64,
}

/// One document of a play snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentData {
    /// Project-relative path.
    pub path: String,
    /// Document kind (`scene`, `prefab`).
    pub kind: String,
    /// Asset ID.
    pub id: String,
    /// Records by ID, as stored.
    pub records: BTreeMap<String, Record>,
}

/// Everything a player needs to build the world: documents, project types, seed,
/// tick rate and the published asset generations it must load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayScene {
    /// Portable native game UI, present in both Play and exported packs.
    #[serde(default)]
    pub ui: Option<GameUi>,
    /// Typed project rendering defaults. A player command-line override wins.
    /// Omitted by older format-1 senders, which retain hardware auto-detection.
    #[serde(default)]
    pub rendering: ara_components::render::RenderSettings,
    /// Format version (1).
    pub format: u32,
    /// Scene asset ID.
    pub scene: String,
    /// Scene and prefab documents.
    pub documents: Vec<DocumentData>,
    /// Project `*.types.json` documents.
    pub types: Vec<Value>,
    /// Session seed.
    pub seed: u64,
    /// Fixed tick rate.
    pub tick_hz: u32,
    /// Asset ID → action digest (64 hex) of the generation to load.
    pub assets: BTreeMap<String, String>,
    /// Content-addressed cache directory holding those generations.
    pub cache: String,
    /// Navmesh tile cache directory.
    pub nav_cache: Option<String>,
    /// C# gameplay modules, when the project has any.
    #[serde(default)]
    pub scripts: Option<ScriptScene>,
    /// Imported asset ID → project path: the asset index the documents' references
    /// resolve against (cooking streamed cells checks them).
    #[serde(default)]
    pub imports: BTreeMap<String, String>,
}

/// Shared quality choices serialized by project settings.
pub use ara_components::render::QualitySetting;
/// Shared typed rendering defaults carried by a Play snapshot.
pub use ara_components::render::RenderSettings;

impl PlayScene {
    /// Captures documents from a workspace (scene and prefabs alike).
    pub fn documents(workspace: &Workspace) -> Vec<DocumentData> {
        workspace
            .docs()
            .map(|(path, doc)| DocumentData {
                path: path.to_owned(),
                kind: doc.kind.clone(),
                id: doc.id.to_string(),
                records: doc
                    .records()
                    .iter()
                    .map(|(id, r)| (id.to_string(), r.clone()))
                    .collect(),
            })
            .collect()
    }

    /// Rebuilds the workspace and scene ID (records are validated by projection).
    pub fn workspace(&self) -> Result<(Workspace, AssetId), String> {
        let mut index = ara_doc::refs::AssetIndex::new();
        for (id, path) in &self.imports {
            index.insert(id.clone(), path.clone(), None);
        }
        let mut workspace = Workspace::new(index);
        for data in &self.documents {
            let mut doc = Document::new(data.kind.clone(), AssetId::parse(&data.id)?);
            for (id, record) in &data.records {
                doc.insert_unchecked(RecordId::parse(id)?, record.clone());
            }
            workspace.insert(&data.path, doc);
        }
        Ok((workspace, AssetId::parse(&self.scene)?))
    }

    /// Binary (CBOR) encoding for the shared-memory section.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(self, &mut bytes).map_err(|e| e.to_string())?;
        Ok(bytes)
    }

    /// Decodes and checks the format version.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let scene: Self = ciborium::de::from_reader(bytes).map_err(|e| e.to_string())?;
        if scene.format != 1 {
            return Err(format!("unsupported play snapshot format {}", scene.format));
        }
        Ok(scene)
    }
}

/// Formats bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses hex text of any even length into bytes (portable snapshots).
pub fn unhex_bytes(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err("hex text must hold whole ASCII bytes".into());
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Parses a 64-character hex digest.
pub fn unhex(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 {
        return Err(format!("expected 64 hex characters, got {}", text.len()));
    }
    let mut out = [0; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod script_tests {
    use super::*;

    #[test]
    fn old_script_scenes_keep_development_and_shipping_has_no_author_paths() {
        let old =
            serde_json::json!({"hostDir":"dev/host","compileServer":null,"modules":["dev/game"]});
        let development: ScriptScene = serde_json::from_value(old).expect("legacy script scene");
        assert_eq!(development.host, ScriptHost::Development);
        let shipping = ScriptScene::shipping("game/scripts.json");
        assert!(
            shipping.host_dir.is_empty()
                && shipping.modules.is_empty()
                && shipping.compile_server.is_none()
        );
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&shipping, &mut encoded).expect("encode shipping host");
        let decoded: ScriptScene =
            ciborium::de::from_reader(encoded.as_slice()).expect("decode shipping host");
        assert_eq!(decoded, shipping);
    }
}
