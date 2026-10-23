//! Bounded, sample-clock voices. Construction and reclamation happen on the control path.

use crate::{
    AudioError, Frame, QUANTUM, Result,
    spatial::{AcousticPath, Spatializer, StereoPanner},
    streaming::StreamReader,
};
use kira::{
    info::Info,
    sound::{Sound, SoundData},
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

mod resident;

/// Immutable resident audio. Cloning a clip shares its samples on the control path.
#[derive(Clone)]
pub struct Clip {
    pub(crate) frames: Arc<[Frame]>,
    sample_rate: u32,
}

impl Clip {
    /// Takes ownership of finite mono/stereo frames, enforcing the resident byte limit.
    pub fn new(sample_rate: u32, frames: Vec<Frame>, max_bytes: usize) -> Result<Self> {
        if !(8000..=192000).contains(&sample_rate) || frames.is_empty() {
            return Err(AudioError::Invalid("clip sample rate or empty clip"));
        }
        if frames.len().saturating_mul(size_of::<Frame>()) > max_bytes {
            return Err(AudioError::Capacity("resident clip budget"));
        }
        if frames
            .iter()
            .any(|f| !f.left.is_finite() || !f.right.is_finite())
        {
            return Err(AudioError::Invalid("non-finite PCM"));
        }
        Ok(Self {
            frames: frames.into(),
            sample_rate,
        })
    }
    /// Source frames per second, before resampling.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Number of stereo frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    /// Clips are nonempty by construction.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    /// Borrow decoded stereo samples for cooking or inspection.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
}

/// Prepared voice controls. All values must be finite; linear gain is in [0,16].
#[derive(Debug, Clone, Copy, ara_schema::Schema, serde::Serialize, serde::Deserialize)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct VoiceParameters {
    /// Linear amplitude, normally source gain times distance/cone attenuation.
    #[ara(default = 1, range(min = 0, max = 16))]
    pub gain: f32,
    /// Playback rate, normally source pitch times Doppler.
    #[ara(default = 1, range(min = 0.03125, max = 32))]
    pub rate: f32,
    /// Unit listener-local direction, with forward along negative Z.
    #[ara(default=[0,0,-1])]
    pub direction: [f32; 3],
    /// Worker-computed occlusion, transmission and reflection send.
    pub path: AcousticPath,
}

impl Default for VoiceParameters {
    fn default() -> Self {
        Self {
            gain: 1.0,
            rate: 1.0,
            direction: [0.0, 0.0, -1.0],
            path: AcousticPath::default(),
        }
    }
}

impl VoiceParameters {
    /// Builds a presentation update from ECS source/listener values and a worker's acoustic result.
    pub fn from_source(
        source: &crate::AudioSource,
        listener: &crate::AudioListener,
        path: AcousticPath,
    ) -> Result<Self> {
        let spatial = crate::spatial::evaluate(source, listener)?;
        let parameters = Self {
            gain: source.gain * spatial.attenuation,
            rate: source.pitch * spatial.doppler,
            direction: spatial.direction,
            path,
        };
        parameters.validate()?;
        Ok(parameters)
    }
    pub(crate) fn validate(self) -> Result<()> {
        if !self.gain.is_finite()
            || !(0.0..=16.0).contains(&self.gain)
            || !self.rate.is_finite()
            || !(0.03125..=32.0).contains(&self.rate)
            || self.direction.iter().any(|v| !v.is_finite())
            || !self
                .direction
                .iter()
                .map(|v| v * v)
                .sum::<f32>()
                .is_finite()
            || self.direction.iter().map(|v| v * v).sum::<f32>() < 0.00001
            || !self.path.visibility.is_finite()
            || !(0.0..=1.0).contains(&self.path.visibility)
            || self
                .path
                .transmission
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.path.reverb_send.is_finite()
            || !(0.0..=1.0).contains(&self.path.reverb_send)
        {
            return Err(AudioError::Invalid("voice parameters"));
        }
        Ok(())
    }
}

/// Starts a resident or streamed voice. The start is exact on the output sample clock.
pub struct Playback {
    /// Absolute frame of the first source sample.
    pub at_frame: u64,
    /// Whether a resident clip wraps; streaming loops are supplied by the producer.
    pub looping: bool,
    /// Higher values win the real voice budget; ties use gain then oldest ID.
    pub priority: u8,
    /// False preserves stereo source channels for music and UI instead of mono spatialization.
    pub spatial: bool,
    /// Initial gain, pitch and acoustic parameters.
    pub parameters: VoiceParameters,
    /// Optional binaural renderer, constructed before enqueueing the sound.
    pub spatializer: Option<Box<dyn Spatializer>>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            at_frame: 0,
            looping: false,
            priority: 128,
            spatial: true,
            parameters: VoiceParameters::default(),
            spatializer: None,
        }
    }
}

impl Playback {
    /// Converts ECS intent to a runtime command. The bridge resolves the clip/bus and optional HRTF separately.
    pub fn from_source(
        source: &crate::AudioSource,
        listener: &crate::AudioListener,
        at_frame: u64,
        path: AcousticPath,
    ) -> Result<Self> {
        Ok(Self {
            at_frame,
            looping: source.looping,
            priority: source.priority,
            spatial: source.spatial,
            parameters: VoiceParameters::from_source(source, listener, path)?,
            spatializer: None,
        })
    }
}

/// A sample-accurate presentation command, not an authoring document mutation.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum VoiceCommand {
    /// Smooth gain and rate starting at an exact frame. Geometry updates at the next quantum.
    Set {
        /// Absolute presentation frame.
        at_frame: u64,
        /// New controls.
        parameters: VoiceParameters,
        /// Linear smoothing duration, in frames; zero is an explicit hard change.
        ramp_frames: u32,
    },
    /// Fade and stop at an exact frame.
    Stop {
        /// First frame of the fade.
        at_frame: u64,
        /// Fade duration; zero stops immediately.
        fade_frames: u32,
    },
}

impl VoiceCommand {
    pub(crate) fn at_frame(self) -> u64 {
        match self {
            Self::Set { at_frame, .. } | Self::Stop { at_frame, .. } => at_frame,
        }
    }
}

pub(crate) struct VoiceShared {
    pub active: AtomicBool,
    pub finished: AtomicBool,
    pub position: AtomicU64,
    pub audibility: AtomicU32,
}

pub(crate) struct VoiceControl {
    pub shared: Arc<VoiceShared>,
    pub commands: Producer<VoiceCommand>,
    pub last_command: u64,
    pub priority: u8,
    pub at_frame: u64,
}

pub(crate) enum Input {
    Resident {
        clip: Clip,
        cursor: usize,
        looping: bool,
    },
    Stream(StreamReader),
}

/// Fully allocated voice, ready to move from a loader to the audio worker at a boundary.
/// Native HRTFs, decoder prebuffering and all voice queue/box allocations happen before installation.
pub struct PreparedVoice {
    pub(crate) sound: Box<Voice>,
    pub(crate) control: VoiceControl,
}
impl PreparedVoice {
    /// Prepares resident playback off the callback. Scheduling is validated again on installation.
    pub fn resident(clip: Clip, playback: Playback) -> Result<Self> {
        playback.parameters.validate()?;
        let input = Input::Resident {
            clip,
            cursor: 0,
            looping: playback.looping,
        };
        Ok(Self::new(input, playback))
    }
    /// Prepares a prebuffered stream off the callback. The producer owns streaming loops.
    pub fn streaming(stream: StreamReader, playback: Playback) -> Result<Self> {
        playback.parameters.validate()?;
        if playback.looping {
            return Err(AudioError::Invalid(
                "stream looping belongs to its decoder worker",
            ));
        }
        Ok(Self::new(Input::Stream(stream), playback))
    }
    fn new(input: Input, playback: Playback) -> Self {
        let (sound, control) = Voice::new(input, playback, 0);
        Self {
            sound: Box::new(sound),
            control,
        }
    }
}
pub(crate) struct ReadySound(pub Box<Voice>);

impl Input {
    fn sample_rate(&self) -> u32 {
        match self {
            Self::Resident { clip, .. } => clip.sample_rate,
            Self::Stream(s) => s.sample_rate(),
        }
    }
    fn next(&mut self) -> Frame {
        match self {
            Self::Resident {
                clip,
                cursor,
                looping,
            } => {
                if *cursor >= clip.len() {
                    if *looping {
                        *cursor = 0;
                    } else {
                        return Frame::ZERO;
                    }
                }
                let frame = clip.frames[*cursor];
                *cursor += 1;
                frame
            }
            Self::Stream(s) => s.next_frame(),
        }
    }
    fn ended(&self) -> bool {
        match self {
            Self::Resident {
                clip,
                cursor,
                looping,
            } => !looping && *cursor >= clip.len(),
            Self::Stream(s) => s.ended(),
        }
    }
}

pub(crate) struct Voice {
    input: Input,
    history: [Frame; 4],
    fraction: f64,
    source_rate: u32,
    time: u64,
    start: u64,
    commands: Consumer<VoiceCommand>,
    pending: Option<VoiceCommand>,
    shared: Arc<VoiceShared>,
    parameters: VoiceParameters,
    gain: Smooth,
    rate: Smooth,
    virtual_gain: Smooth,
    stop: Option<u64>,
    tail: usize,
    ended: bool,
    dry_ended: bool,
    first_process: bool,
    mono: [f32; QUANTUM],
    stereo: [Frame; QUANTUM],
    spatial: bool,
    spatializer: Box<dyn Spatializer>,
}

impl Voice {
    pub(crate) fn align_clock(&mut self, frame: u64) {
        self.time = frame;
    }
    pub(crate) fn new(mut input: Input, playback: Playback, time: u64) -> (Self, VoiceControl) {
        let (commands, reader) = RingBuffer::new(64);
        let shared = Arc::new(VoiceShared {
            active: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            position: AtomicU64::new(0),
            audibility: AtomicU32::new(playback.parameters.gain.to_bits()),
        });
        let control = VoiceControl {
            shared: Arc::clone(&shared),
            commands,
            last_command: time,
            priority: playback.priority,
            at_frame: playback.at_frame,
        };
        let rate = input.sample_rate();
        let first = input.next();
        let history = [first, first, input.next(), input.next()];
        let spatializer = playback
            .spatializer
            .unwrap_or_else(|| Box::<StereoPanner>::default());
        let tail = 4;
        (
            Self {
                input,
                history,
                fraction: 0.0,
                source_rate: rate,
                time,
                start: playback.at_frame,
                commands: reader,
                pending: None,
                shared,
                parameters: playback.parameters,
                gain: Smooth::new(playback.parameters.gain),
                rate: Smooth::new(playback.parameters.rate),
                virtual_gain: Smooth::new(0.0),
                stop: None,
                tail,
                ended: false,
                dry_ended: false,
                first_process: true,
                mono: [0.0; QUANTUM],
                stereo: [Frame::ZERO; QUANTUM],
                spatial: playback.spatial,
                spatializer,
            },
            control,
        )
    }

    fn apply_commands(&mut self) {
        loop {
            if self.pending.is_none() {
                self.pending = self.commands.pop().ok();
            }
            let Some(command) = self.pending else {
                break;
            };
            if command.at_frame() > self.time {
                break;
            }
            self.pending = None;
            match command {
                VoiceCommand::Set {
                    parameters,
                    ramp_frames,
                    ..
                } => {
                    self.gain.set(parameters.gain, ramp_frames);
                    self.rate.set(parameters.rate, ramp_frames);
                    self.parameters = parameters;
                }
                VoiceCommand::Stop { fade_frames, .. } => {
                    self.gain.set(0.0, fade_frames);
                    self.stop = Some(self.time + u64::from(fade_frames));
                }
            }
        }
    }
}

impl SoundData for ReadySound {
    type Error = Infallible;
    type Handle = ();
    fn into_sound(self) -> std::result::Result<(Box<dyn Sound>, ()), Infallible> {
        Ok((self.0, ()))
    }
}

impl Sound for Voice {
    fn process(&mut self, out: &mut [Frame], dt: f64, _: &Info) {
        let target = if self.shared.active.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        if self.first_process && self.time + out.len() as u64 > self.start {
            self.virtual_gain = Smooth::new(target);
            self.first_process = false;
        }
        if self.virtual_gain.target != target {
            self.virtual_gain.set(target, 64);
        }
        self.apply_commands();
        if self.resident_block(out, dt, target) {
            return;
        }
        let spatial = self.parameters;
        let draining = self.dry_ended;
        self.mono.fill(0.0);
        self.stereo.fill(Frame::ZERO);
        let mut advanced = 0;
        for index in 0..out.len() {
            if self.pending.is_some_and(|c| c.at_frame() <= self.time) {
                self.apply_commands();
            }
            if self.time < self.start && self.stop.is_some_and(|stop| self.time >= stop) {
                self.dry_ended = true;
            }
            if self.time >= self.start && !self.dry_ended {
                let sample = cubic(self.history, self.fraction as f32);
                let gain = self.gain.next() * self.virtual_gain.next();
                self.mono[index] = (sample.left + sample.right) * 0.5 * gain;
                self.stereo[index] = sample * gain;
                let step = self.rate.next() as f64 * self.source_rate as f64 * dt;
                self.fraction += step;
                while self.fraction >= 1.0 {
                    self.fraction -= 1.0;
                    self.history.rotate_left(1);
                    self.history[3] = self.input.next();
                }
                if self.input.ended() || self.stop.is_some_and(|stop| self.time >= stop) {
                    self.tail = self.tail.saturating_sub(1);
                    if self.tail == 0 {
                        self.dry_ended = true;
                    }
                }
                advanced += 1;
            }
            self.time += 1;
        }
        self.shared.position.fetch_add(advanced, Ordering::Relaxed);
        self.shared
            .audibility
            .store(self.gain.value.to_bits(), Ordering::Relaxed);
        if self.spatial && (target > 0.0 || self.virtual_gain.value > 0.0) {
            if draining {
                self.ended = self.spatializer.drain(out);
            } else {
                self.spatializer.process(
                    &self.mono[..out.len()],
                    out,
                    spatial.direction,
                    spatial.path,
                );
            }
        } else if !self.spatial {
            out.copy_from_slice(&self.stereo[..out.len()]);
            self.ended = self.dry_ended;
        } else {
            out.fill(Frame::ZERO);
            self.ended = self.dry_ended;
        }
        if self.ended {
            self.shared.finished.store(true, Ordering::Release);
        }
    }
    fn finished(&self) -> bool {
        self.ended
    }
}

pub(crate) struct Smooth {
    value: f32,
    pub target: f32,
    step: f32,
    remaining: u32,
}
impl Smooth {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }
    pub fn set(&mut self, target: f32, frames: u32) {
        self.target = target;
        self.remaining = frames;
        if frames == 0 {
            self.value = target;
        } else {
            self.step = (target - self.value) / frames as f32;
        }
    }
    pub fn next(&mut self) -> f32 {
        let value = self.value;
        if self.remaining > 0 {
            self.value += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.value = self.target;
            }
        }
        value
    }
}

fn cubic(f: [Frame; 4], t: f32) -> Frame {
    let interpolate = |a: f32, b: f32, c: f32, d: f32| {
        b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
    };
    Frame::new(
        interpolate(f[0].left, f[1].left, f[2].left, f[3].left),
        interpolate(f[0].right, f[1].right, f[2].right, f[3].right),
    )
}
