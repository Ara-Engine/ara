//! Engine Protocol params of the play and test methods (schema-validated by the host).
use ara_schema::Schema;
use serde::Deserialize;
use serde_json::Value;

/// Params of `play.start`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayStartParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Scene path or asset ID; default: the project's main scene.
    pub scene: Option<String>,
    /// Return a starting world immediately and prepare it off the caller thread.
    /// Poll `play.status` for progress/readiness; `play.stop` cancels a pending
    /// start. Omitted/false retains the synchronous world-ready response.
    pub background: Option<bool>,
    /// `local` (default), `host` (Listen) or `dedicated`.
    pub mode: Option<String>,
    /// Execute isolated player worlds in one shared process or separate processes (default).
    pub process_mode: Option<ara_play::ProcessMode>,
    /// Additional client worlds for `host`/`dedicated` (max 16).
    pub clients: Option<u32>,
    /// Open windows (SDL3) for local/host/client players; headless otherwise.
    pub window: Option<bool>,
    /// Show every rendering player (local, host, clients; not a dedicated server) in
    /// the editor through a shared GPU surface of this size instead of a window
    /// (implies no window). Each player in the result gets a `surface`; send input
    /// with `play.device` and resize with `play.resize`.
    pub embed: Option<crate::embed::EmbedParams>,
    /// Start paused at tick 0 (step explicitly); default runs at the fixed rate.
    pub start_paused: Option<bool>,
    /// Session seed for every deterministic random stream (default 0).
    pub seed: Option<u64>,
    /// Fixed tick rate in Hz (default 60; must divide 48000).
    pub tick_hz: Option<u32>,
    /// Push newly imported asset generations into this world while it plays.
    pub hot_reload: Option<bool>,
    /// Per-client simulated link conditions for `host`/`dedicated`, in client order:
    /// `{latencyMs, jitterMs, loss, duplication, reordering, bandwidth}` (probabilities
    /// in basis points, bandwidth in bytes/s) or null for a clean link.
    pub conditions: Option<Vec<Value>>,
}

/// Params of methods addressing one world.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
}

/// Params of `play.step`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayStepParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Fixed ticks to run as fast as possible.
    pub ticks: u32,
    /// Action injections `{action, value, holdTicks, afterTicks, player}` relative to the first tick.
    pub inputs: Option<Vec<Value>>,
}

/// Params of `play.input`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayInputParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Action injections `{action, value, holdTicks, afterTicks, player}` for the next tick.
    pub inputs: Vec<Value>,
}

/// Params of `play.device`: raw input for one embedded player.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayDeviceParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// The one player that gets the events (0 = the local or host player, 1.. =
    /// clients in launch order, player `index + 1`).
    pub player: u32,
    /// Events in order: `{kind: "key", scancode, down}` (SDL scancodes: W 26, A 4,
    /// S 22, D 7, arrows 79-82, Space 44, Left Shift 225), `{kind: "mouseButton",
    /// button, down}` (1 left, 2 middle, 3 right; hold 3 to look), `{kind:
    /// "mouseMotion", dx, dy}` (relative pixels) or `{kind: "focusLost"}` (releases
    /// every held key and stops looking). Mapped to Move/Jump/Sprint/Look exactly
    /// like a windowed player's keyboard and mouse.
    pub events: Vec<Value>,
}

/// Params of `play.resize`: a new size for one embedded player's surface.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayResizeParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// The embedded player (0 = the local or host player, 1.. = clients).
    pub player: u32,
    /// New width in pixels.
    #[ara(range(min = 1, max = 8192))]
    pub width: u32,
    /// New height in pixels.
    #[ara(range(min = 1, max = 8192))]
    pub height: u32,
}

/// Params of `play.inspect`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayInspectParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Entity address (`e_fox`).
    pub entity: String,
    /// Components to read; all present when absent.
    pub components: Option<Vec<String>>,
    /// Networked worlds: which process answers (0 = the authority, the default;
    /// 1.. = clients in launch order, player `index + 1`).
    pub player: Option<u32>,
    /// Return a Host job immediately; inspect the result through jobs instead of blocking.
    pub background: Option<bool>,
}

/// Params of `play.read`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayReadParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// `entity.Component.field` paths (`e_fox.Transform.position.z`).
    pub paths: Vec<String>,
    /// Networked worlds: which process answers (0 = the authority, the default;
    /// 1.. = clients in launch order, player `index + 1`).
    pub player: Option<u32>,
    /// Return a Host job immediately while the requested player answers.
    pub background: Option<bool>,
}

/// Params of `play.patch`: live edits applied at the next tick boundary.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayPatchParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Edits `{entity, path, value}` (`{"entity":"e_lamp1","path":"Light.color","value":[1,0,0,1]}`).
    pub sets: Vec<Value>,
    /// Return a Host job immediately; completion acknowledges the applying authority tick.
    pub background: Option<bool>,
}

/// Parameters of the nonblocking editor view switch `play.view`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayViewParams {
    /// Calling editor or agent session.
    pub session: String,
    /// Active world; absent selects the latest world.
    pub world: Option<String>,
    /// Rendering player index (0 local/listen host, 1+ clients).
    pub player: u32,
    /// Read the current typed navigation state; performs no navigation mutation.
    pub inspect: Option<bool>,
    /// Editor spectator camera; null restores this player's gameplay camera.
    pub camera: Option<ara_play::ViewCamera>,
    /// Start spectator mode from this player's current gameplay camera.
    pub capture_current: Option<bool>,
    /// Spectator navigation speed in metres per second; default 4, valid range 0.5–50.
    #[ara(range(min = 0.5, max = 50))]
    pub navigation_speed: Option<f32>,
}
/// Parameters of `play.control`: one authority-boundary possession command.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayControlParams {
    /// Calling editor or agent session.
    pub session: String,
    /// Active world; absent selects the latest world.
    pub world: Option<String>,
    /// Player index whose controls transfer (0 local/listen host, 1+ clients).
    pub player: u32,
    /// Controllable entity address; null ejects this player without stopping simulation.
    pub entity: Option<ara_schema::EntityRef>,
}

/// Params of `play.capture`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayCaptureParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// PNG to write (also `.marks.png` and `.legend.json`); state hash only when absent.
    pub path: Option<String>,
    /// Width in pixels (default 1280).
    pub width: Option<u32>,
    /// Height in pixels (default 720).
    pub height: Option<u32>,
    /// Maximum numbered marks (default 40; 0 disables).
    pub marks: Option<u32>,
    /// Networked worlds: which process answers (0 = the authority, the default;
    /// 1.. = clients in launch order, player `index + 1`).
    pub player: Option<u32>,
    /// Render what windowed frames draw (terrain chunks as meshes; no landscape or
    /// VFX composite) instead of the full capture (default false).
    pub presented: Option<bool>,
}

/// Params of `play.watch`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayWatchParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// `entity.Component.field` paths sampled by step/until.
    pub paths: Vec<String>,
    /// Sampling interval in ticks (default 1).
    pub every: Option<u32>,
}

/// Params of `play.until`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayUntilParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// `entity.Component.field` path.
    pub path: String,
    /// `eq`, `ne`, `gt`, `ge`, `lt` or `le`.
    pub compare: String,
    /// Right-hand value.
    pub value: Value,
    /// Tick budget.
    pub max_ticks: u32,
    /// Action injections relative to the first tick.
    pub inputs: Option<Vec<Value>>,
}

/// Params of `play.timescale`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayTimescaleParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Host-time multiplier in [0, 1000]; the fixed dt never changes.
    pub value: f64,
}

/// Params of `play.recording` and `play.replay`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayFileParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Replay file path.
    pub path: String,
}

/// Params of the read-only runtime diff and Keep eligibility preview.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayDiffParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World identity; omitted uses the latest world.
    pub world: Option<String>,
    /// Restrict the preview to these exact entity addresses.
    pub entities: Option<Vec<String>>,
    /// Read the runtime boundary through a nonblocking Host job.
    pub background: Option<bool>,
}

/// One exact entity/field pair explicitly selected for Keep Changes.
#[derive(Debug, Clone, PartialEq, Eq, Schema, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayKeepField {
    /// Stable scene entity or prefab address.
    pub entity: String,
    /// Exact reflected field path; this selector never matches a prefix.
    pub path: String,
}

/// Params of `play.keep`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayKeepParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Only these entities.
    pub entities: Option<Vec<String>>,
    /// Only paths starting with one of these (`Light.color`).
    pub paths: Option<Vec<String>>,
    /// Exact selected entity/field pairs, intersected with any legacy filters.
    /// An explicitly empty list keeps nothing.
    pub fields: Option<Vec<PlayKeepField>>,
    /// Also keep values runtime systems write every tick (poses, velocities).
    pub include_driven: Option<bool>,
    /// Why, shown in review.
    pub why: Option<String>,
    /// Read the runtime diff asynchronously, then keep it in one attributed content transaction.
    pub background: Option<bool>,
}

/// Params of `play.debug.crash` (fault-isolation tests).
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayCrashParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// `panic`, `access` or `hang`.
    pub kind: String,
}

/// Params of `play.pool`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayPoolParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Idle headless players to keep warm.
    pub headless: Option<u32>,
    /// Idle windowed players to keep warm.
    pub windowed: Option<u32>,
}

/// Params of `play.status`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayStatusParams {
    /// Session handle from `session.begin`.
    pub session: String,
}

/// Params of `test.run`: a playtest in a fresh headless player.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestRunParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Scene path or asset ID; default: the project's main scene.
    pub scene: Option<String>,
    /// Session seed.
    pub seed: Option<u64>,
    /// Fixed tick rate in Hz.
    pub tick_hz: Option<u32>,
    /// Steps `{do: input|step|until|expect|patch|watch|capture|hash, …}` run in order.
    pub steps: Vec<Value>,
    /// Run twice in separate processes and require equal final state hashes.
    pub determinism: Option<bool>,
}

/// Params of `play.command`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayCommandParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// C# command name (`game.UseMedkit`).
    pub name: String,
    /// Command params; entity params may use document addresses (`{"actor": "e_fox"}`).
    pub params: Option<Value>,
}

/// Params of `play.captureAudio`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayCaptureAudioParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// WAV file to write; the analysis (loudness, peaks, spectrum) goes beside it
    /// as `<name>.audio.json`.
    pub path: String,
    /// Seconds of sound to render offline from the current tick.
    #[ara(unit = Seconds, range(min = 0.01, max = 60))]
    pub seconds: f64,
}

/// Params of `script.write`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptWriteParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// Project-relative `.cs` path inside a module folder (`scripts/Outpost/Door.cs`).
    pub path: String,
    /// The complete new source text.
    pub text: String,
}

/// Params of `play.restart`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayRestartParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// World ID from `play.start` (`play:1`); default: the most recent world.
    pub world: Option<String>,
    /// Resume from a portable snapshot of the current tick (C# state included) instead
    /// of the start snapshot (what C# L4 restarts do).
    pub keep_state: Option<bool>,
}
