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
//! - `MusicSource` entities play their `.music` score (one `MusicPlayer` each,
//!   following gameplay `intensity`/`segment`); `AudioGraphSource` entities play a
//!   procedural audio graph voice (compiled once per graph, instantiated per entity
//!   with a seed from its identity) spatialized by their `AudioSource`, with
//!   gameplay parameters and triggers forwarded at the next control frame.
//! - [`AudioPresenter::stream_cells`]: baked `AcousticCell` probes stream with the
//!   world's resident cells (decoded on a worker thread, installed at a later update).
//! - [`AudioPresenter::pump`] feeds a device (live play); [`AudioPresenter::capture`]
//!   renders offline (deterministic evidence).
//!
//! Presentation only: nothing here writes the simulation.
use ara_acoustics::bake::{AcousticCell, CellKey};
use ara_acoustics::{
    AcousticMaterial, AcousticPortal, AcousticSource, AcousticSurface, Acoustics,
    AcousticsSettings, Emitter, Evaluation, ReverbZone, RoomReverbHandle,
    physics::{PhysicsRayBuffers, PhysicsScene, SurfaceLookup},
    room_reverb,
};
use ara_audio::{
    AudioEngine, AudioError, AudioListener, AudioSource, BusId, Clip, EngineConfig, Playback,
    PreparedVoice, VoiceCommand, VoiceId,
    capture::AudioCapture,
    device::{DeviceBackend, DeviceEvent},
    diagnose::Audibility,
};
use ara_audio_graph::{GraphHandle, Program};
use ara_audio_mix::{BusDesc, Mixer, MixerDesc, MixerSettings};
use ara_diag::Diagnostic;
use ara_doc::Workspace;
use ara_music::{MusicDesc, MusicPlayer, StemLoader};
use ara_physics::{PhysicsId, PhysicsWorld, QueryFilter};
use ara_physics_jolt::JoltBackend;
use ara_runtime::components::{AudioGraphSource, MusicSource};
use ara_runtime::sync::WorldPose;
use ara_schema::AssetRef;
use ara_sim::Identity;
use bevy_ecs::world::World;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
};

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

/// One playing score and the gameplay values it last followed.
struct Music {
    player: MusicPlayer,
    intensity: f32,
    segment: String,
}

/// A graph voice's control handle and the gameplay inputs it last forwarded.
struct GraphControl {
    handle: GraphHandle,
    parameters: BTreeMap<String, f32>,
    triggers: BTreeMap<String, u32>,
}

/// Prepares music stems through the presenter's clip loader (resident voices).
struct Stems<'a>(&'a mut dyn FnMut(&AssetRef) -> Result<Clip>);

impl StemLoader for Stems<'_> {
    fn prepare(&mut self, clip: &str, playback: Playback) -> Result<PreparedVoice> {
        PreparedVoice::resident((self.0)(&AssetRef(clip.to_owned()))?, playback)
    }
}

/// A deterministic voice seed from an entity identity (FNV-1a).
fn seed(entity: &str) -> u64 {
    entity.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A decoded cell (or why it failed), sent from its decode thread.
type Decoded = (String, Result<AcousticCell>);

/// Baked probes of streamed cells: installed keys, decodes in flight and their results.
struct Cells {
    installed: BTreeMap<String, Option<CellKey>>,
    decoded: (Sender<Decoded>, Receiver<Decoded>),
}

/// Loads a cell's baked probes from its published `ara.acoustics` import.
fn acoustic_cell(
    assets: &dyn ara_runtime::assets::AssetSource,
    reference: &AssetRef,
) -> Result<AcousticCell> {
    let asset = ara_runtime::assets::asset_id(reference).map_err(|e| invalid(e.0))?;
    let (_, cached) = assets
        .load(asset)
        .map_err(|e| invalid(e.0))?
        .ok_or_else(|| invalid(format!("acoustics {} are not imported", reference.0)))?;
    let bytes = cached
        .output
        .artifacts
        .iter()
        .find(|a| a.name == "acoustics.bin")
        .ok_or_else(|| invalid(format!("{} has no acoustics.bin", reference.0)))?;
    AcousticCell::decode(&bytes.bytes)
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
    cells: Cells,
    scores: BTreeMap<String, MusicDesc>,
    graph_documents: BTreeMap<String, ara_graph::Graph>,
    programs: BTreeMap<String, std::sync::Arc<Program>>,
    music: BTreeMap<String, Music>,
    graphs: BTreeMap<String, GraphControl>,
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
        let mut scores = BTreeMap::new();
        let mut graph_documents = BTreeMap::new();
        for (path, doc) in documents.docs() {
            match doc.kind.as_str() {
                "music" => {
                    let desc = MusicDesc::from_document(doc).map_err(|d| {
                        invalid(format!("{path}: {}", d.first().map_or("", |d| &d.message)))
                    })?;
                    scores.insert(doc.id.to_string(), desc);
                }
                "graph" => {
                    let graph = ara_graph::Graph::from_document(doc)
                        .map_err(|d| invalid(format!("{path}: {}", d.message)))?;
                    if graph.header.domain == "audio" {
                        graph_documents.insert(doc.id.to_string(), graph);
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            engine,
            mixer,
            room,
            acoustics: Acoustics::new(settings)?,
            voices: BTreeMap::new(),
            buffers: PhysicsRayBuffers::default(),
            evaluations: Vec::new(),
            cells: Cells {
                installed: BTreeMap::new(),
                decoded: channel(),
            },
            scores,
            graph_documents,
            programs: BTreeMap::new(),
            music: BTreeMap::new(),
            graphs: BTreeMap::new(),
        })
    }

    /// The compiled program of an audio graph document (compiled once, shared).
    fn program(
        &mut self,
        reference: &AssetRef,
        clips: &mut dyn FnMut(&AssetRef) -> Result<Clip>,
    ) -> Result<std::sync::Arc<Program>> {
        let id = ara_runtime::assets::asset_id(reference).map_err(|e| invalid(e.0))?;
        if let Some(program) = self.programs.get(id) {
            return Ok(program.clone());
        }
        let graph = self
            .graph_documents
            .get(id)
            .ok_or_else(|| invalid(format!("{} is not an audio graph", reference.0)))?;
        let first = |d: Vec<Diagnostic>| {
            invalid(format!(
                "{}: {}",
                reference.0,
                d.first().map_or("invalid audio graph", |d| &d.message)
            ))
        };
        let audio = ara_audio_graph::AudioGraph::from_graph(graph).map_err(first)?;
        let checked = ara_audio_graph::check(&audio).map_err(first)?;
        let mut source = |clip: &str| clips(&AssetRef(clip.to_owned()));
        let program = ara_audio_graph::compile(&checked, self.engine.sample_rate(), &mut source)
            .map_err(first)?;
        self.programs.insert(id.to_owned(), program.clone());
        Ok(program)
    }

    /// Starts, follows and stops `MusicSource` scores.
    fn update_music(
        &mut self,
        sources: &[(String, MusicSource)],
        clips: &mut dyn FnMut(&AssetRef) -> Result<Clip>,
    ) -> Result<()> {
        let gone: Vec<String> = self
            .music
            .keys()
            .filter(|e| !sources.iter().any(|(s, _)| s == *e))
            .cloned()
            .collect();
        for entity in gone {
            if let Some(mut music) = self.music.remove(&entity) {
                let at = self.engine.next_frame();
                music.player.stop(&mut self.engine, at, 4800);
            }
        }
        let mut stems = Stems(clips);
        for (entity, source) in sources {
            let Some(reference) = &source.music else {
                continue;
            };
            if !self.music.contains_key(entity) {
                let id = ara_runtime::assets::asset_id(reference).map_err(|e| invalid(e.0))?;
                let desc = self
                    .scores
                    .get(id)
                    .ok_or_else(|| invalid(format!("{} is not a music document", reference.0)))?
                    .clone();
                let bus = self.mixer.bus(&source.bus).unwrap_or(BusId::MASTER);
                let mut player = MusicPlayer::new(desc, bus, self.engine.sample_rate());
                player.set_intensity(&mut self.engine, source.intensity)?;
                let at = self.engine.next_frame();
                let first = (!source.segment.is_empty()).then_some(source.segment.as_str());
                player.start(&mut self.engine, &mut stems, first, at)?;
                self.music.insert(
                    entity.clone(),
                    Music {
                        player,
                        intensity: source.intensity,
                        segment: source.segment.clone(),
                    },
                );
                continue;
            }
            let Some(music) = self.music.get_mut(entity) else {
                continue;
            };
            if music.intensity != source.intensity {
                music
                    .player
                    .set_intensity(&mut self.engine, source.intensity)?;
                music.intensity = source.intensity;
            }
            if music.segment != source.segment {
                if !source.segment.is_empty() {
                    music
                        .player
                        .transition(&mut self.engine, &mut stems, &source.segment)?;
                }
                music.segment.clone_from(&source.segment);
            }
            music.player.update(&mut self.engine, &mut stems)?;
        }
        Ok(())
    }

    /// Forwards changed graph parameters and trigger increments of a playing voice.
    fn follow_graph(&mut self, entity: &str, source: &AudioGraphSource) -> Result<()> {
        let at = self.engine.next_frame();
        let Some(control) = self.graphs.get_mut(entity) else {
            return Ok(());
        };
        for (name, &value) in &source.parameters {
            if control.parameters.get(name) != Some(&value) {
                control.handle.set(name, value, at)?;
                control.parameters.insert(name.clone(), value);
            }
        }
        for (name, &count) in &source.triggers {
            let last = control.triggers.get(name).copied().unwrap_or(0);
            if count > last {
                control.handle.trigger(name, at)?;
                control.triggers.insert(name.clone(), count);
            }
        }
        Ok(())
    }

    /// Streams baked acoustic probes with the world. `resident` lists the resident
    /// cells that have probes (cell ID, `Cell.acoustics` import). New cells load and
    /// decode on a worker thread and install at a later [`AudioPresenter::update`];
    /// cells no longer resident unload now. Returns the probes resident afterwards.
    pub fn stream_cells(
        &mut self,
        resident: &[(String, AssetRef)],
        assets: &Arc<dyn ara_runtime::assets::AssetSource>,
    ) -> usize {
        let gone: Vec<String> = self
            .cells
            .installed
            .keys()
            .filter(|cell| !resident.iter().any(|(c, _)| c == *cell))
            .cloned()
            .collect();
        for cell in gone {
            if let Some(Some(key)) = self.cells.installed.remove(&cell) {
                self.acoustics.probes_mut().unload(key);
            }
        }
        for (cell, reference) in resident {
            if self.cells.installed.contains_key(cell) {
                continue;
            }
            // Pending until the worker reports; a failed decode stays absent.
            self.cells.installed.insert(cell.clone(), None);
            let (tx, assets, cell, reference) = (
                self.cells.decoded.0.clone(),
                Arc::clone(assets),
                cell.clone(),
                reference.clone(),
            );
            std::thread::spawn(move || {
                let decoded = acoustic_cell(assets.as_ref(), &reference);
                let _ = tx.send((cell, decoded));
            });
        }
        self.install_cells();
        self.acoustics.probes_mut().probes()
    }

    /// Blocks until every cell decode in flight has installed (or `timeout`): an
    /// offline capture right after [`AudioPresenter::stream_cells`] hears baked probes.
    pub fn wait_cells(&mut self, timeout: std::time::Duration) {
        let start = std::time::Instant::now();
        while self.cells.installed.values().any(Option::is_none) && start.elapsed() < timeout {
            match self
                .cells
                .decoded
                .1
                .recv_timeout(timeout.saturating_sub(start.elapsed()))
            {
                Ok(done) => {
                    // Re-queue through the normal install path.
                    let _ = self.cells.decoded.0.send(done);
                    self.install_cells();
                }
                Err(_) => break,
            }
        }
    }

    /// Installs decoded cells that are still wanted.
    fn install_cells(&mut self) {
        while let Ok((cell, decoded)) = self.cells.decoded.1.try_recv() {
            let Some(slot) = self.cells.installed.get_mut(&cell) else {
                continue;
            };
            if let Ok(decoded) = decoded {
                *slot = Some(decoded.key);
                self.acoustics.probes_mut().load(decoded);
            }
        }
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
        self.install_cells();
        let mut zones = Vec::new();
        let mut portals = Vec::new();
        let mut sources = Vec::new();
        let mut scores = Vec::new();
        let mut graph_sources = BTreeMap::new();
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
            if let Some(music) = entity.get::<MusicSource>() {
                scores.push((id.0.clone(), music.clone()));
            }
            if let Some(graph) = entity.get::<AudioGraphSource>() {
                graph_sources.insert(id.0.clone(), graph.clone());
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
        scores.sort_by(|a, b| a.0.cmp(&b.0));
        self.update_music(&scores, clips)?;
        let at_frame = self.engine.next_frame();
        let mut heard = Vec::new();
        let evaluations = std::mem::take(&mut self.evaluations);
        for ((entity, source, _), evaluation) in sources.iter().zip(&evaluations) {
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
            } else if let Some(graph) = graph_sources.get(entity)
                && let Some(reference) = &graph.graph
            {
                match self.program(reference, clips) {
                    Ok(program) => {
                        let (voice, handle) = program.instantiate(seed(entity));
                        let mut playback =
                            Playback::from_source(source, listener, at_frame, Default::default())?;
                        playback.parameters = evaluation.parameters;
                        playback.looping = false;
                        let bus = self.mixer.bus(&source.bus).unwrap_or(BusId::MASTER);
                        let prepared = PreparedVoice::procedural(Box::new(voice), playback)?;
                        let voice = self.engine.install(bus, prepared)?;
                        self.voices.insert(entity.clone(), voice);
                        self.graphs.insert(
                            entity.clone(),
                            GraphControl {
                                handle,
                                parameters: BTreeMap::new(),
                                triggers: BTreeMap::new(),
                            },
                        );
                    }
                    Err(_) => loaded = false,
                }
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
            if let Some(graph) = graph_sources.get(entity) {
                self.follow_graph(entity, graph)?;
            }
        }
        self.evaluations = evaluations;
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
