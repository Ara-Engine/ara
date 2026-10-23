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
    /// `local` (default), `host` (Listen) or `dedicated`.
    pub mode: Option<String>,
    /// Additional client processes for `host`/`dedicated` (max 16).
    pub clients: Option<u32>,
    /// Open windows (SDL3) for local/host/client players; headless otherwise.
    pub window: Option<bool>,
    /// Start paused at tick 0 (step explicitly); default runs at the fixed rate.
    pub start_paused: Option<bool>,
    /// Session seed for every deterministic random stream (default 0).
    pub seed: Option<u64>,
    /// Fixed tick rate in Hz (default 60; must divide 48000).
    pub tick_hz: Option<u32>,
    /// Push newly imported asset generations into this world while it plays.
    pub hot_reload: Option<bool>,
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
    /// Also keep values runtime systems write every tick (poses, velocities).
    pub include_driven: Option<bool>,
    /// Why, shown in review.
    pub why: Option<String>,
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
    /// WAV file to write; the analysis (loudness, peaks, spectrum) goes beside it as JSON.
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
