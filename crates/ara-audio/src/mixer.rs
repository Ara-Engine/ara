//! Kira graph ownership and Ara voice policy. All mutation methods are control-path APIs.

use crate::{
    AudioError, Clip, Frame, Playback, QUANTUM, Result, VoiceCommand,
    effects::{Meter, PeakLimiter, TrackBuilder},
    streaming::StreamReader,
    voice::{PreparedVoice, ReadySound, VoiceControl},
};
use kira::{
    AudioManager, AudioManagerSettings, Capacities, Tween,
    backend::{Backend, Renderer},
    track::{MainTrackBuilder, SendTrackBuilder, SendTrackHandle, SendTrackId, TrackHandle},
};
use std::{convert::Infallible, sync::atomic::Ordering, time::Duration};

/// Graph-local bus handle. Bus zero is always `master`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct BusId(pub(crate) usize);
impl BusId {
    /// Default output bus, containing every submix.
    pub const MASTER: Self = Self(0);
}
/// Monotonic presentation voice ID; never reused during an engine's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct VoiceId(pub(crate) u64);
impl ara_schema::FieldType for VoiceId {
    fn value_type() -> ara_schema::ValueType {
        ara_schema::ValueType::U64
    }
}

/// Fixed capacities and sample rate, chosen before the audio thread starts.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Output frames per second. SDL resamples if hardware has another rate.
    pub sample_rate: u32,
    /// Maximum audible voices across every bus.
    pub real_voices: usize,
    /// Maximum real plus virtual voices. This also bounds every bus's sound capacity.
    pub total_voices: usize,
    /// Maximum number of buses, including master.
    pub buses: usize,
    /// Optional final limiter ceiling. None exposes Kira's hard-clip boundary for testing.
    pub limiter: Option<f32>,
}
impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            real_voices: 128,
            total_voices: 1024,
            buses: 32,
            limiter: Some(0.98),
        }
    }
}

/// Named bus gains. Applying a snapshot validates every entry before changing any bus.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    /// Application label for tools and diagnostics.
    pub name: String,
    /// Target linear gains, in [0,16]. Duplicate buses are rejected.
    pub gains: Vec<(BusId, f32)>,
}

/// An owned Kira backend. No mutex, cpal, OS clock or worker is involved in rendering.
pub struct PullBackend {
    renderer: Option<Renderer>,
}
impl Backend for PullBackend {
    type Settings = u32;
    type Error = Infallible;
    fn setup(sample_rate: u32, _: usize) -> std::result::Result<(Self, u32), Infallible> {
        Ok((Self { renderer: None }, sample_rate))
    }
    fn start(&mut self, renderer: Renderer) -> std::result::Result<(), Infallible> {
        self.renderer = Some(renderer);
        Ok(())
    }
}
impl PullBackend {
    fn render(&mut self, out: &mut [f32]) {
        if let Some(renderer) = &mut self.renderer {
            renderer.on_start_processing();
            renderer.process(out, 2);
        }
    }
}

struct Bus {
    name: String,
    handle: TrackHandle,
    meter: Meter,
    gain: f32,
}
struct ControlledVoice {
    id: VoiceId,
    control: VoiceControl,
}

/// A single-owner audio engine. Put it on a dedicated presentation worker in a live player.
/// Loading/cooking, HRTF creation and graph edits must occur outside a device callback.
pub struct AudioEngine {
    manager: AudioManager<PullBackend>,
    config: EngineConfig,
    buses: Vec<Bus>,
    voices: Vec<ControlledVoice>,
    candidates: Vec<usize>,
    next_id: u64,
    generated: u64,
    delivered: u64,
    cache: [f32; QUANTUM * 2],
    cache_cursor: usize,
    master_meter: Meter,
    cache_clipped: [u64; 4],
    delivered_overloads: u64,
    sends: Vec<SendTrackHandle>,
}

impl AudioEngine {
    /// Allocates graph capacities and DSP buffers. Does not open a device.
    pub fn new(config: EngineConfig) -> Result<Self> {
        if !(8000..=192000).contains(&config.sample_rate)
            || config.real_voices == 0
            || config.total_voices < config.real_voices
            || config.total_voices > 65536
            || config.buses == 0
            || config.buses > 256
        {
            return Err(AudioError::Invalid("engine capacities or sample rate"));
        }
        let meter = Meter::default();
        let mut main = MainTrackBuilder::new().sound_capacity(0);
        main.add_built_effect(meter.effect());
        if let Some(ceiling) = config.limiter {
            main.add_built_effect(Box::new(PeakLimiter::new(ceiling, 0.1)?));
        }
        let mut manager = AudioManager::<PullBackend>::new(AudioManagerSettings {
            capacities: Capacities {
                sub_track_capacity: config.buses,
                send_track_capacity: config.buses,
                ..Capacities::default()
            },
            main_track_builder: main,
            internal_buffer_size: QUANTUM,
            backend_settings: config.sample_rate,
        })
        .map_err(|never| -> AudioError { match never {} })?;
        let bus_meter = Meter::default();
        let mut builder = TrackBuilder::new()
            .sound_capacity(config.total_voices)
            .sub_track_capacity(config.buses);
        builder.add_built_effect(bus_meter.effect());
        let root = manager
            .add_sub_track(builder)
            .map_err(|_| AudioError::Capacity("buses"))?;
        let mut buses = Vec::with_capacity(config.buses);
        buses.push(Bus {
            name: "master".into(),
            handle: root,
            meter: bus_meter,
            gain: 1.0,
        });
        Ok(Self {
            manager,
            config,
            buses,
            voices: Vec::with_capacity(config.total_voices),
            candidates: Vec::with_capacity(config.total_voices),
            next_id: 1,
            generated: 0,
            delivered: 0,
            cache: [0.0; QUANTUM * 2],
            cache_cursor: QUANTUM * 2,
            master_meter: meter,
            cache_clipped: [0; 4],
            delivered_overloads: 0,
            sends: Vec::with_capacity(config.buses),
        })
    }

    /// Graph's fixed sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }
    /// First frame not yet generated. Schedule commands at this frame or later.
    pub fn next_frame(&self) -> u64 {
        self.generated
    }
    /// Number of frames delivered to the offline/device consumer.
    pub fn delivered_frames(&self) -> u64 {
        self.delivered
    }
    /// Meter before final limiting. This reveals overloads even when output is protected.
    pub fn master_meter(&self) -> &Meter {
        &self.master_meter
    }
    /// Overloaded pre-limiter samples corresponding exactly to delivered frames, including partial quanta.
    pub fn delivered_overloads(&self) -> u64 {
        self.delivered_overloads
    }

    /// Adds a submix beneath a previously registered bus. Keep the returned ID in the bridge.
    pub fn add_bus(
        &mut self,
        name: impl Into<String>,
        parent: BusId,
        mut builder: TrackBuilder,
    ) -> Result<BusId> {
        let name = name.into();
        if self.buses.len() >= self.config.buses {
            return Err(AudioError::Capacity("buses"));
        }
        if name.is_empty() || self.buses.iter().any(|b| b.name == name) {
            return Err(AudioError::Invalid("unique bus name required"));
        }
        let meter = Meter::default();
        builder.add_built_effect(meter.effect());
        let builder = builder
            .sound_capacity(self.config.total_voices)
            .sub_track_capacity(self.config.buses);
        let parent = self
            .buses
            .get_mut(parent.0)
            .ok_or(AudioError::UnknownHandle)?;
        let handle = parent
            .handle
            .add_sub_track(builder)
            .map_err(|_| AudioError::Capacity("submixes"))?;
        let id = BusId(self.buses.len());
        self.buses.push(Bus {
            name,
            handle,
            meter,
            gain: 1.0,
        });
        Ok(id)
    }

    /// Resolves a source's reflected bus name.
    pub fn bus(&self, name: &str) -> Option<BusId> {
        self.buses.iter().position(|b| b.name == name).map(BusId)
    }
    /// Obtains a sidechain meter while configuring buses.
    pub fn bus_meter(&self, id: BusId) -> Result<&Meter> {
        self.buses
            .get(id.0)
            .map(|b| &b.meter)
            .ok_or(AudioError::UnknownHandle)
    }
    /// Adds a shared effect return. Connect a bus using TrackBuilder::with_send before add_bus.
    pub fn add_send(&mut self, builder: SendTrackBuilder) -> Result<SendTrackId> {
        if self.sends.len() >= self.config.buses {
            return Err(AudioError::Capacity("effect sends"));
        }
        let handle = self
            .manager
            .add_send_track(builder)
            .map_err(|_| AudioError::Capacity("effect sends"))?;
        let id = handle.id();
        self.sends.push(handle);
        Ok(id)
    }
    /// Smooths an existing bus-to-send route, including worker-computed AcousticPath::reverb_send.
    pub fn set_send(
        &mut self,
        bus: BusId,
        send: SendTrackId,
        gain: f32,
        frames: u32,
    ) -> Result<()> {
        if !gain.is_finite() || !(0.0..=16.0).contains(&gain) {
            return Err(AudioError::Invalid("effect send gain"));
        }
        let db = if gain == 0.0 {
            kira::Decibels::SILENCE.0
        } else {
            20.0 * gain.log10()
        };
        self.buses
            .get_mut(bus.0)
            .ok_or(AudioError::UnknownHandle)?
            .handle
            .set_send(
                send,
                db,
                Tween {
                    duration: Duration::from_secs_f64(
                        frames as f64 / self.config.sample_rate as f64,
                    ),
                    ..Tween::default()
                },
            )
            .map_err(|_| AudioError::UnknownHandle)
    }
    /// Captures bus target gains on the control path, ready to restore as a named snapshot.
    pub fn snapshot(&self, name: impl Into<String>) -> Snapshot {
        Snapshot {
            name: name.into(),
            gains: self
                .buses
                .iter()
                .enumerate()
                .map(|(i, b)| (BusId(i), b.gain))
                .collect(),
        }
    }
    /// Tweens every bus in a snapshot. Source gain commands remain sample-accurate independently.
    pub fn apply_snapshot(&mut self, snapshot: &Snapshot, frames: u32) -> Result<()> {
        for (index, (id, gain)) in snapshot.gains.iter().enumerate() {
            if id.0 >= self.buses.len()
                || !gain.is_finite()
                || !(0.0..=16.0).contains(gain)
                || snapshot.gains[..index].iter().any(|(other, _)| other == id)
            {
                return Err(AudioError::Invalid("snapshot gain or bus"));
            }
        }
        for &(id, gain) in &snapshot.gains {
            self.buses[id.0].gain = gain;
            let db = if gain == 0.0 {
                kira::Decibels::SILENCE.0
            } else {
                20.0 * gain.log10()
            };
            self.buses[id.0].handle.set_volume(
                db,
                Tween {
                    duration: Duration::from_secs_f64(
                        frames as f64 / self.config.sample_rate as f64,
                    ),
                    ..Tween::default()
                },
            );
        }
        Ok(())
    }

    /// Starts an immutable resident clip; sharing/cloning samples belongs on this control path.
    pub fn play(&mut self, bus: BusId, clip: Clip, playback: Playback) -> Result<VoiceId> {
        self.install(bus, PreparedVoice::resident(clip, playback)?)
    }
    /// Starts a prebuffered, bounded stream. Its decoder lives on a separate worker.
    pub fn play_stream(
        &mut self,
        bus: BusId,
        stream: StreamReader,
        playback: Playback,
    ) -> Result<VoiceId> {
        self.install(bus, PreparedVoice::streaming(stream, playback)?)
    }
    /// Installs a loader-prepared voice without constructing DSP or allocating a sound/queue.
    /// Invoke between render calls; completed Kira resources may be reclaimed on this control path.
    pub fn install(&mut self, bus: BusId, mut prepared: PreparedVoice) -> Result<VoiceId> {
        if prepared.control.at_frame < self.generated {
            return Err(AudioError::Invalid("play command is in the past"));
        }
        self.collect_finished();
        if self.voices.len() >= self.config.total_voices {
            return Err(AudioError::Capacity("virtual voices"));
        }
        let id = VoiceId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(AudioError::Capacity("voice IDs"))?;
        prepared.sound.align_clock(self.generated);
        prepared.control.last_command = self.generated;
        self.buses
            .get_mut(bus.0)
            .ok_or(AudioError::UnknownHandle)?
            .handle
            .play(ReadySound(prepared.sound))
            .map_err(|_| AudioError::Capacity("bus voices"))?;
        self.voices.push(ControlledVoice {
            id,
            control: prepared.control,
        });
        Ok(id)
    }

    /// Queues an ordered command; rejects past/out-of-order commands and queue overflow.
    pub fn command(&mut self, id: VoiceId, command: VoiceCommand) -> Result<()> {
        if command.at_frame() < self.generated {
            return Err(AudioError::Invalid("command is in the past"));
        }
        if let VoiceCommand::Set { parameters, .. } = command {
            parameters.validate()?;
        }
        let v = self
            .voices
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or(AudioError::UnknownHandle)?;
        if v.control.shared.finished.load(Ordering::Acquire) {
            return Err(AudioError::UnknownHandle);
        }
        if command.at_frame() < v.control.last_command {
            return Err(AudioError::Invalid("commands must be ordered per voice"));
        }
        v.control
            .commands
            .push(command)
            .map_err(|_| AudioError::Capacity("voice commands"))?;
        v.control.last_command = command.at_frame();
        Ok(())
    }
    /// Output frames elapsed for an active or virtual voice. No simulation state is exposed.
    pub fn voice_position(&self, id: VoiceId) -> Option<u64> {
        self.voices
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.control.shared.position.load(Ordering::Relaxed))
    }
    /// Releases completed control handles; call outside the callback to reclaim memory.
    pub fn collect_finished(&mut self) {
        self.voices
            .retain(|v| !v.control.shared.finished.load(Ordering::Acquire));
    }

    fn rebalance(&mut self) {
        self.candidates.clear();
        for (i, v) in self.voices.iter().enumerate() {
            v.control.shared.active.store(false, Ordering::Relaxed);
            if !v.control.shared.finished.load(Ordering::Acquire)
                && v.control.at_frame < self.generated + QUANTUM as u64
            {
                self.candidates.push(i);
            }
        }
        self.candidates.sort_unstable_by(|&a, &b| {
            let a = &self.voices[a];
            let b = &self.voices[b];
            b.control
                .priority
                .cmp(&a.control.priority)
                .then_with(|| {
                    f32::from_bits(b.control.shared.audibility.load(Ordering::Relaxed)).total_cmp(
                        &f32::from_bits(a.control.shared.audibility.load(Ordering::Relaxed)),
                    )
                })
                .then(a.id.0.cmp(&b.id.0))
        });
        for &i in self.candidates.iter().take(self.config.real_voices) {
            self.voices[i]
                .control
                .shared
                .active
                .store(true, Ordering::Relaxed);
        }
    }

    /// Renders arbitrary-length interleaved stereo without allocating or using a wall clock.
    /// Fixed quanta make results independent of the caller's buffer partitioning.
    pub fn render(&mut self, out: &mut [f32]) -> Result<()> {
        if !out.len().is_multiple_of(2) {
            return Err(AudioError::Invalid("stereo buffer length"));
        }
        let mut cursor = 0;
        while cursor < out.len() {
            if self.cache_cursor == self.cache.len() {
                self.rebalance();
                self.manager.backend_mut().render(&mut self.cache);
                self.cache_clipped = self.master_meter.clipped_mask();
                self.generated += QUANTUM as u64;
                self.cache_cursor = 0;
            }
            let n = (out.len() - cursor).min(self.cache.len() - self.cache_cursor);
            out[cursor..cursor + n]
                .copy_from_slice(&self.cache[self.cache_cursor..self.cache_cursor + n]);
            for i in self.cache_cursor..self.cache_cursor + n {
                self.delivered_overloads += (self.cache_clipped[i / 64] >> (i % 64)) & 1;
            }
            self.cache_cursor += n;
            cursor += n;
        }
        self.delivered += (out.len() / 2) as u64;
        Ok(())
    }

    /// Convenience for agent code rendering structured frames. This allocates on the caller.
    pub fn render_frames(&mut self, frames: usize) -> Result<Vec<Frame>> {
        let mut interleaved = vec![
            0.0;
            frames
                .checked_mul(2)
                .ok_or(AudioError::Capacity("render size"))?
        ];
        self.render(&mut interleaved)?;
        Ok(interleaved
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| Frame::new(f[0], f[1]))
            .collect())
    }
}
