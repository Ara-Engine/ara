//! Reflected authoring components and protocol parameters; independent of ECS.

use ara_schema::{AssetRef, Schema, SchemaDb};
use serde::{Deserialize, Serialize};

/// Distance rolloff model, evaluated in meters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Attenuation {
    /// Inverse distance, clamped at the reference distance and silent at the maximum.
    #[default]
    Inverse,
    /// Linear fade between reference and maximum distance.
    Linear,
    /// Inverse-square energy falloff with a finite range fade.
    InverseSquare,
}

/// Declarative sound intent. Playback cursors and decoder state are presentation only.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::component::Component))]
#[ara(component, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AudioSource {
    /// Cooked clip reference in `@path#id` form; absent means no clip.
    #[ara(asset_kind = "audio")]
    pub clip: Option<AssetRef>,
    /// Mixer bus name resolved by the presentation bridge.
    #[ara(default = "master")]
    pub bus: String,
    /// Linear source amplitude.
    #[ara(default = 1, range(min = 0, max = 16))]
    pub gain: f32,
    /// Playback pitch multiplier before Doppler.
    #[ara(default = 1, range(min = 0.125, max = 8))]
    pub pitch: f32,
    /// Whether the clip repeats.
    pub looping: bool,
    /// Whether position affects the sound.
    #[ara(default = true)]
    pub spatial: bool,
    /// Voice budget priority; larger values are retained first.
    #[ara(default = 128)]
    pub priority: u8,
    /// Full-volume radius.
    #[ara(unit = Meters, default = 1, range(min = 0.01, max = 100000))]
    pub reference_distance: f32,
    /// Silent beyond this distance; must exceed referenceDistance.
    #[ara(unit = Meters, default = 100, range(min = 0.02, max = 1000000))]
    pub max_distance: f32,
    /// Shape of the distance attenuation curve.
    pub attenuation: Attenuation,
    /// Source position, supplied from the ECS world transform by the bridge.
    #[ara(unit = Meters)]
    pub position: [f32; 3],
    /// Source velocity in meters per second for Doppler.
    pub velocity: [f32; 3],
    /// Source forward axis; normalized during spatial evaluation.
    #[ara(default = [0, 0, -1])]
    pub forward: [f32; 3],
    /// Full angle of the unattenuated source cone.
    #[ara(unit = Degrees, default = 360, range(min = 0, max = 360))]
    pub cone_inner: f32,
    /// Full angle at which the source reaches coneOuterGain.
    #[ara(unit = Degrees, default = 360, range(min = 0, max = 360))]
    pub cone_outer: f32,
    /// Linear amplitude outside the source cone.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub cone_outer_gain: f32,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            clip: None,
            bus: "master".into(),
            gain: 1.0,
            pitch: 1.0,
            looping: false,
            spatial: true,
            priority: 128,
            reference_distance: 1.0,
            max_distance: 100.0,
            attenuation: Attenuation::Inverse,
            position: [0.0; 3],
            velocity: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            cone_inner: 360.0,
            cone_outer: 360.0,
            cone_outer_gain: 1.0,
        }
    }
}

/// Presentation listener, usually copied from the local player's camera component.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::component::Component))]
#[ara(component, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AudioListener {
    /// Listener world position.
    #[ara(unit = Meters)]
    pub position: [f32; 3],
    /// Listener world velocity in meters per second.
    pub velocity: [f32; 3],
    /// Forward direction; the default engine basis looks down negative Z.
    #[ara(default = [0, 0, -1])]
    pub forward: [f32; 3],
    /// Up direction; must not be parallel to forward.
    #[ara(default = [0, 1, 0])]
    pub up: [f32; 3],
    /// Listener sensitivity cone inner full angle.
    #[ara(unit = Degrees, default = 360, range(min = 0, max = 360))]
    pub cone_inner: f32,
    /// Listener sensitivity cone outer full angle.
    #[ara(unit = Degrees, default = 360, range(min = 0, max = 360))]
    pub cone_outer: f32,
    /// Linear sensitivity outside the listener cone.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub cone_outer_gain: f32,
}

impl Default for AudioListener {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            velocity: [0.0; 3],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            cone_inner: 360.0,
            cone_outer: 360.0,
            cone_outer_gain: 1.0,
        }
    }
}

/// Schema-discoverable `audio.play` parameters; protocol resolves asset and entity handles.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct PlayAudioParams {
    /// Source entity address in the current presentation world.
    pub entity: ara_schema::EntityRef,
    /// Absolute output frame on the presentation sample clock.
    pub at_frame: u64,
}

/// Parameters for the offline `capture_audio` job, run outside the human's audio engine.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params, module = "audio")]
pub struct CaptureAudioParams {
    /// Capture duration; bounded so an agent cannot allocate an unbounded buffer.
    #[ara(unit = Seconds, default = 1, range(min = 0.001, max = 60))]
    pub seconds: f64,
}

/// Schema-discoverable `audio.control` parameters for an existing presentation voice.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct ControlAudioParams {
    /// Session-local runtime voice handle returned by audio.play, never a network entity ID.
    pub voice: crate::VoiceId,
    /// Absolute presentation sample frame at which gain/rate automation begins.
    pub at_frame: u64,
    /// Validated source gain/rate and listener-local acoustic parameters.
    pub parameters: crate::VoiceParameters,
    /// Linear transition length in output frames; zero requests an explicit hard change.
    #[ara(default = 64)]
    pub ramp_frames: u32,
}

/// Schema-discoverable `audio.stop` parameters; this stops presentation only.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct StopAudioParams {
    /// Session-local runtime voice handle.
    pub voice: crate::VoiceId,
    /// Absolute presentation sample frame at which stopping begins.
    pub at_frame: u64,
    /// Dry-source fade length in output frames; native DSP tails drain afterwards.
    #[ara(default = 64)]
    pub fade_frames: u32,
}

/// Registers audio components and agent parameters. Fields have no replication policy.
pub fn register(db: &mut SchemaDb) {
    db.register::<AudioSource>()
        .register::<AudioListener>()
        .register::<PlayAudioParams>()
        .register::<CaptureAudioParams>()
        .register::<ControlAudioParams>()
        .register::<StopAudioParams>();
}
