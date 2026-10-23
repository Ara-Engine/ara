//! Audio in play on ara-audio v2. [`AudioPresenter`] owns one engine per world:
//!
//! - Setup: the room-reverb aux (`ara-acoustics`), then the mixer (`ara-audio-mix`)
//!   from the project's `.mixer` document, else `master` routed to the room reverb.
//! - [`AudioPresenter::update`] (presentation rate, never the audio thread): reverb
//!   zones and portals from their components at world poses, every `AudioSource`
//!   evaluated against the Jolt physics scene (occlusion, transmission, distance, air,
//!   Doppler, reverb sends) in one batched trace; new voices start on their mixer bus,
//!   existing ones retarget with `VoiceCommand::Set`; the room reverb follows the
//!   listener. Returns audibility reports (`ara why` causes) for sources not heard.
//! - [`AudioPresenter::pump`] feeds a device (live play); [`AudioPresenter::capture`]
//!   renders offline (deterministic evidence).
//!
//! Presentation only: nothing here writes the simulation.
use ara_acoustics::{
    AcousticMaterial, AcousticPortal, AcousticSource, AcousticSurface, Acoustics,
    AcousticsSettings, Emitter, Evaluation, ReverbZone, RoomReverbHandle,
    physics::{PhysicsRayBuffers, PhysicsScene, SurfaceLookup},
    room_reverb,
};
use ara_audio::{
    AudioEngine, AudioError, AudioListener, AudioSource, BusId, Clip, EngineConfig, Playback,
    VoiceCommand, VoiceId,
    capture::AudioCapture,
    device::{DeviceBackend, DeviceEvent},
    diagnose::Audibility,
};
use ara_audio_mix::{BusDesc, Mixer, MixerDesc, MixerSettings};
use ara_diag::Diagnostic;
use ara_doc::Workspace;
use ara_physics::{PhysicsId, PhysicsWorld, QueryFilter};
use ara_physics_jolt::JoltBackend;
use ara_runtime::sync::WorldPose;
use ara_schema::AssetRef;
use ara_sim::Identity;
use bevy_ecs::world::World;
use std::collections::BTreeMap;

/// Errors of the audio presentation path.
pub type Result<T> = std::result::Result<T, AudioError>;

/// Frames over which retargeted voice parameters ramp (5.3 ms at 48 kHz).
const RAMP: u32 = 256;

/// How one source was heard by the latest update.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    /// Entity address.
    pub entity: String,
    /// Source-listener distance in metres.
    pub distance: f32,
    /// Distance/cone attenuation, excluding source gain.
    pub attenuation: f32,
    /// Unblocked fraction of the direct path.
    pub visibility: f32,
    /// What blocks the direct path, nearest first (`concrete e_wall01`).
    pub occluders: Vec<String>,
    /// Why it is (partly) inaudible; empty when it is heard.
    pub explain: Vec<Diagnostic>,
}

/// Physics user data (`PhysicsId`) to acoustic surfaces and labels.
struct Surfaces(BTreeMap<u64, (AcousticMaterial, String)>);

impl SurfaceLookup for Surfaces {
    fn material(&self, user_data: u64) -> AcousticMaterial {
        self.0
            .get(&user_data)
            .map_or_else(AcousticMaterial::default, |(m, _)| *m)
    }
    fn label(&self, user_data: u64) -> String {
        self.0
            .get(&user_data)
            .map_or_else(|| format!("#{user_data}"), |(_, l)| l.clone())
    }
}

/// Loads a source's clip from its published `ara.audio` import (`audio.bin` +
/// `audio.json`): cooked PCM/ADPCM WAV or Ogg Opus, decoded resident.
pub fn clip(assets: &dyn ara_runtime::assets::AssetSource, reference: &AssetRef) -> Result<Clip> {
    let asset = ara_runtime::assets::asset_id(reference).map_err(|e| invalid(e.0))?;
    let (_, cached) = assets
        .load(asset)
        .map_err(|e| invalid(e.0))?
        .ok_or_else(|| invalid(format!("sound {} is not imported", reference.0)))?;
    let artifact = |name: &str| {
        cached
            .output
            .artifacts
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.bytes.clone())
            .ok_or_else(|| invalid(format!("sound {} has no {name}", reference.0)))
    };
    let meta: serde_json::Value = serde_json::from_slice(&artifact("audio.json")?)
        .map_err(|e| invalid(format!("sound {}: {e}", reference.0)))?;
    let opus = meta["codec"] == "opus";
    let bytes = std::io::Cursor::new(artifact("audio.bin")?);
    let mut decoder: Box<dyn ara_audio::codec::AudioDecoder> = if opus {
        Box::new(ara_audio::codec::OpusDecoder::open(bytes)?)
    } else {
        Box::new(ara_audio::codec::MediaDecoder::open(
            Box::new(bytes),
            "wav",
        )?)
    };
    ara_audio::codec::decode_clip(decoder.as_mut(), 64 * 1024 * 1024)
}

/// One world's audio: engine, mixer, room reverb, acoustics and live voices.
pub struct AudioPresenter {
    engine: AudioEngine,
    mixer: Mixer,
    room: RoomReverbHandle,
    acoustics: Acoustics,
    voices: BTreeMap<String, VoiceId>,
    buffers: PhysicsRayBuffers,
    evaluations: Vec<Evaluation>,
}

fn invalid(what: String) -> AudioError {
    AudioError::Backend(what)
}

impl AudioPresenter {
    /// An engine at 48 kHz with the room reverb aux and the mixer of the first `mixer`
    /// document in `documents` (none: `master` routed to the room reverb).
    pub fn new(documents: &Workspace) -> Result<Self> {
        let config = EngineConfig::default();
        let mut engine = AudioEngine::new(config)?;
        let settings = AcousticsSettings::default();
        let (reverb, room) = room_reverb(engine.sample_rate(), &settings);
        let aux = engine.add_aux("room", Box::new(reverb))?;
        let desc = match documents.docs().find(|(_, d)| d.kind == "mixer") {
            Some((path, doc)) => MixerDesc::from_document(doc).map_err(|d| {
                invalid(format!("{path}: {}", d.first().map_or("", |d| &d.message)))
            })?,
            None => MixerDesc::new(
                vec![BusDesc {
                    name: "master".into(),
                    reverb: true,
                    ..Default::default()
                }],
                Vec::new(),
                MixerSettings::default(),
            )
            .map_err(|d| invalid(d.first().map_or_else(String::new, |d| d.message.clone())))?,
        };
        let mixer = Mixer::build(&mut engine, desc, Some(aux)).map_err(|d| invalid(d.message))?;
        Ok(Self {
            engine,
            mixer,
            room,
            acoustics: Acoustics::new(settings)?,
            voices: BTreeMap::new(),
            buffers: PhysicsRayBuffers::default(),
            evaluations: Vec::new(),
        })
    }

    /// The engine (meters, voice states, mixer states).
    pub fn engine(&mut self) -> &mut AudioEngine {
        &mut self.engine
    }

    /// One presentation update: zones, acoustics, voices and the room reverb. `clips`
    /// loads a source's clip (resident). Returns how every source is heard.
    pub fn update(
        &mut self,
        world: &World,
        listener: &AudioListener,
        clips: &mut dyn FnMut(&AssetRef) -> Result<Clip>,
    ) -> Result<Vec<Heard>> {
        let mut zones = Vec::new();
        let mut portals = Vec::new();
        let mut sources = Vec::new();
        let mut surfaces = BTreeMap::new();
        for entity in world.iter_entities() {
            let Some(id) = entity.get::<Identity>() else {
                continue;
            };
            let pose = entity.get::<WorldPose>().map(|p| p.0);
            let position = pose.map_or([0.0; 3], |p| p.translation.to_array());
            if let Some(zone) = entity.get::<ReverbZone>() {
                let mut zone = zone.clone();
                zone.position = position;
                if let Some(p) = pose {
                    zone.rotation = p.rotation.to_array();
                }
                zones.push(zone);
            }
            if let Some(portal) = entity.get::<AcousticPortal>() {
                let mut portal = portal.clone();
                portal.position = position;
                portals.push(portal);
            }
            if let Some(physics) = entity.get::<PhysicsId>() {
                let material = entity
                    .get::<AcousticSurface>()
                    .map_or_else(AcousticMaterial::default, AcousticSurface::material);
                surfaces.insert(physics.value, (material, id.0.clone()));
            }
            if let Some(source) = entity.get::<AudioSource>() {
                let mut source = source.clone();
                source.position = position;
                if let Some(v) = entity.get::<ara_physics::Velocity>() {
                    source.velocity = v.linear;
                }
                let acoustic = entity.get::<AcousticSource>().cloned().unwrap_or_default();
                sources.push((id.0.clone(), source, acoustic));
            }
        }
        // Stable identity order: equal worlds evaluate equally.
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        self.acoustics.set_zones(&zones, &portals)?;
        let emitters: Vec<Emitter<'_>> = sources
            .iter()
            .map(|(label, source, acoustic)| Emitter {
                label,
                source,
                acoustic,
            })
            .collect();
        let surfaces = Surfaces(surfaces);
        match world.get_resource::<PhysicsWorld<JoltBackend>>() {
            Some(physics) => {
                let mut scene = PhysicsScene {
                    backend: physics.backend(),
                    filter: QueryFilter::default(),
                    surfaces: &surfaces,
                    buffers: &mut self.buffers,
                };
                self.acoustics
                    .evaluate(&mut scene, listener, &emitters, &mut self.evaluations)?;
            }
            None => {
                let mut open = ara_acoustics::MeshScene::default();
                self.acoustics
                    .evaluate(&mut open, listener, &emitters, &mut self.evaluations)?;
            }
        }
        let at_frame = self.engine.next_frame();
        let mut heard = Vec::new();
        for ((entity, source, _), evaluation) in sources.iter().zip(&self.evaluations) {
            let mut loaded = true;
            if let Some(&voice) = self.voices.get(entity) {
                self.engine.command(
                    voice,
                    VoiceCommand::Set {
                        at_frame,
                        parameters: evaluation.parameters,
                        ramp_frames: RAMP,
                    },
                )?;
            } else if let Some(reference) = &source.clip {
                match clips(reference) {
                    Ok(clip) => {
                        let mut playback =
                            Playback::from_source(source, listener, at_frame, Default::default())?;
                        playback.parameters = evaluation.parameters;
                        let bus = self.mixer.bus(&source.bus).unwrap_or(BusId::MASTER);
                        let voice = self.engine.play(bus, clip, playback)?;
                        self.voices.insert(entity.clone(), voice);
                    }
                    Err(_) => loaded = false,
                }
            }
            let occluders: Vec<String> = evaluation
                .record
                .blockers
                .iter()
                .map(|b| format!("{} {}", b.material, b.entity))
                .collect();
            let report = Audibility {
                entity: entity.clone(),
                source: source.clone(),
                loaded,
                voice: self
                    .voices
                    .get(entity)
                    .and_then(|v| self.engine.voice_state(*v)),
                muted_bus: None,
                distance: evaluation.distance,
                attenuation: evaluation.attenuation,
                path: evaluation.parameters.path,
                occluders: occluders.clone(),
            };
            heard.push(Heard {
                entity: entity.clone(),
                distance: evaluation.distance,
                attenuation: evaluation.attenuation,
                visibility: evaluation.record.visibility,
                occluders,
                explain: report.explain(),
            });
        }
        self.room.set(&self.acoustics.room(listener))?;
        Ok(heard)
    }

    /// Feeds a device to `target_frames` queued frames (live play; call every frame).
    pub fn pump(
        &mut self,
        backend: &mut dyn DeviceBackend,
        target_frames: usize,
    ) -> Result<DeviceEvent> {
        ara_audio::device::pump(&mut self.engine, backend, target_frames)
    }

    /// Renders `seconds` offline from the current state (a deterministic capture).
    pub fn capture(&mut self, seconds: f64) -> Result<AudioCapture> {
        ara_audio::capture::capture_audio(&mut self.engine, seconds)
    }
}
