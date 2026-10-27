//! The interactive music player. A control-path object: it schedules sample-clock voices
//! ahead of time (`Playback::at_frame`) and never touches the audio thread directly.

use crate::{
    desc::{MusicDesc, MusicSync, StemDesc, TransitionDesc},
    grid::{Grid, Position},
};
use ara_audio::{
    AudioEngine, AudioError, BusId, Playback, PreparedVoice, Result, VoiceCommand, VoiceId,
    VoiceParameters,
};

/// Prepares stem and stinger voices (resident clips or prebuffered streams) on a loader.
pub trait StemLoader {
    /// A voice for `clip` (an `@path#id` reference) with the given playback.
    fn prepare(&mut self, clip: &str, playback: Playback) -> Result<PreparedVoice>;
}

/// A segment start, as scheduled.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scheduled {
    /// Segment name.
    pub segment: String,
    /// Output frame of its first sample.
    pub frame: u64,
}

/// Where the music is.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicPosition {
    /// Segment name.
    pub segment: String,
    /// Bar and beat in the segment.
    pub at: Position,
}

struct Stem {
    stem: usize,
    id: VoiceId,
    last: u64,
}

struct Play {
    segment: usize,
    start: u64,
    end: u64,
    stems: Vec<Stem>,
    /// Intensity the stem gains were last set for.
    intensity: f32,
}

/// Interactive music: segments, layers, transitions, stingers and resequencing.
pub struct MusicPlayer {
    desc: MusicDesc,
    grids: Vec<Grid>,
    bus: BusId,
    rate: u32,
    rng: u64,
    intensity: f32,
    previous: Option<(usize, u64)>,
    current: Option<Play>,
    next: Option<Play>,
    then: Option<usize>,
    lookahead: u64,
    log: Vec<Scheduled>,
}

/// How many scheduled segment starts `history` keeps.
pub const HISTORY: usize = 256;

fn linear(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f32.powf(db / 20.0).min(16.0)
    }
}

/// Smoothstep weight of a stem's intensity window.
fn weight(window: [f32; 2], intensity: f32) -> f32 {
    let [lo, hi] = window;
    if hi <= lo {
        return if intensity >= lo { 1.0 } else { 0.0 };
    }
    let t = ((intensity - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl MusicPlayer {
    /// A stopped player on `bus` (usually a music bus with a -16 LUFS target).
    pub fn new(desc: MusicDesc, bus: BusId, sample_rate: u32) -> Self {
        let grids = desc
            .segments
            .iter()
            .map(|s| Grid::new(s, sample_rate))
            .collect();
        Self {
            rng: u64::from(desc.settings.seed) ^ 0x6d75_7369_63a5_a5a5,
            intensity: desc.settings.intensity.clamp(0.0, 1.0),
            desc,
            grids,
            bus,
            rate: sample_rate,
            previous: None,
            current: None,
            next: None,
            then: None,
            lookahead: u64::from(sample_rate / 2),
            log: Vec::new(),
        }
    }

    /// Segment starts so far (newest last, at most `HISTORY`).
    pub fn history(&self) -> &[Scheduled] {
        &self.log
    }

    /// Current intensity.
    pub fn intensity(&self) -> f32 {
        self.intensity
    }

    fn gain(&self, stem: &StemDesc) -> f32 {
        linear(stem.gain_db) * weight(stem.intensity, self.intensity)
    }

    fn parameters(gain: f32) -> VoiceParameters {
        VoiceParameters {
            gain,
            ..VoiceParameters::default()
        }
    }

    fn schedule(
        &mut self,
        engine: &mut AudioEngine,
        loader: &mut dyn StemLoader,
        segment: usize,
        at: u64,
        fade_in: u32,
    ) -> Result<Play> {
        let desc = &self.desc.segments[segment];
        let mut stems = Vec::with_capacity(desc.stems.len());
        for (i, stem) in desc.stems.iter().enumerate() {
            let gain = self.gain(stem);
            let playback = Playback {
                at_frame: at,
                spatial: false,
                priority: 250,
                looping: stem.looping,
                parameters: Self::parameters(if fade_in > 0 { 0.0 } else { gain }),
                ..Playback::default()
            };
            let clip = stem
                .clip
                .as_ref()
                .ok_or(AudioError::Invalid("music stem without a clip"))?;
            let mut prepared = loader.prepare(&clip.0, playback)?;
            if desc.loudness.is_some() {
                prepared = prepared.with_loudness(desc.loudness, None);
            }
            let id = engine.install(self.bus, prepared)?;
            if fade_in > 0 {
                engine.command(
                    id,
                    VoiceCommand::Set {
                        at_frame: at,
                        parameters: Self::parameters(gain),
                        ramp_frames: fade_in,
                    },
                )?;
            }
            stems.push(Stem {
                stem: i,
                id,
                last: at,
            });
        }
        if self.log.len() >= HISTORY {
            self.log.remove(0);
        }
        self.log.push(Scheduled {
            segment: desc.name.clone(),
            frame: at,
        });
        Ok(Play {
            segment,
            start: at,
            end: at + self.grids[segment].length().round() as u64,
            stems,
            intensity: self.intensity,
        })
    }

    fn follow_up(&mut self, segment: usize) -> usize {
        let next = &self.desc.segments[segment].next;
        let total: f32 = next.iter().map(|n| n.weight).sum();
        if next.is_empty() || total <= 0.0 {
            return segment;
        }
        // SplitMix64 for deterministic, seedable resequencing.
        self.rng = self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let mut pick = ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32 * total;
        for n in next {
            if pick < n.weight {
                return self.desc.segment(&n.segment).unwrap_or(segment);
            }
            pick -= n.weight;
        }
        self.desc
            .segment(&next[next.len() - 1].segment)
            .unwrap_or(segment)
    }

    fn segment(&self, name: Option<&str>) -> Result<usize> {
        let name = name.unwrap_or(&self.desc.settings.initial);
        self.desc
            .segment(name)
            .ok_or(AudioError::Invalid("unknown music segment"))
    }

    /// Starts `segment` (or the initial segment) at `at_frame`, replacing any music.
    pub fn start(
        &mut self,
        engine: &mut AudioEngine,
        loader: &mut dyn StemLoader,
        segment: Option<&str>,
        at_frame: u64,
    ) -> Result<()> {
        let index = self.segment(segment)?;
        self.stop(engine, at_frame, 0);
        self.current = Some(self.schedule(engine, loader, index, at_frame, 0)?);
        Ok(())
    }

    /// Stops all music from `at_frame` (no earlier than the next rendered frame) with a fade.
    pub fn stop(&mut self, engine: &mut AudioEngine, at_frame: u64, fade_frames: u32) {
        let at_frame = at_frame.max(engine.next_frame());
        for play in [self.current.take(), self.next.take()]
            .into_iter()
            .flatten()
        {
            for s in play.stems {
                let at = at_frame.max(s.last);
                // Finished voices are already gone; nothing to stop.
                let _ = engine.command(
                    s.id,
                    VoiceCommand::Stop {
                        at_frame: at,
                        fade_frames,
                    },
                );
            }
        }
        self.then = None;
    }

    /// Call every control update: promotes the queued segment and schedules the
    /// follow-up (resequencing) half a second before the current segment ends.
    pub fn update(&mut self, engine: &mut AudioEngine, loader: &mut dyn StemLoader) -> Result<()> {
        self.promote(engine)?;
        let now = engine.next_frame();
        let due = self
            .current
            .as_ref()
            .filter(|c| self.next.is_none() && c.end <= now + self.lookahead)
            .map(|c| (c.segment, c.end));
        if let Some((segment, end)) = due {
            let target = match self.then {
                Some(t) => t,
                None => self.follow_up(segment),
            };
            // A late update (long render, stall) starts the follow-up now rather than failing.
            self.next = Some(self.schedule(engine, loader, target, end.max(now), 0)?);
            self.then = None;
        }
        Ok(())
    }

    /// Makes a queued segment current once it has started, and brings stems that were
    /// scheduled before an intensity change up to date.
    fn promote(&mut self, engine: &mut AudioEngine) -> Result<()> {
        let now = engine.next_frame();
        if self.next.as_ref().is_some_and(|n| n.start <= now) {
            if let Some(c) = &self.current {
                self.previous = Some((c.segment, c.start));
            }
            self.current = self.next.take();
        }
        self.refresh(engine)
    }

    /// Sends layer gains to started segments whose intensity is stale. Segments that have
    /// not started yet are refreshed by `promote`, so their command queues never grow.
    fn refresh(&mut self, engine: &mut AudioEngine) -> Result<()> {
        let now = engine.next_frame();
        let ramp = (self.desc.settings.layer_fade * self.rate as f32) as u32;
        let intensity = self.intensity;
        for play in [self.current.as_mut(), self.next.as_mut()]
            .into_iter()
            .flatten()
            .filter(|p| p.start <= now && p.intensity != intensity)
        {
            let segment = &self.desc.segments[play.segment];
            for s in &mut play.stems {
                let stem = &segment.stems[s.stem];
                let gain = linear(stem.gain_db) * weight(stem.intensity, intensity);
                let at = now.max(s.last);
                let command = VoiceCommand::Set {
                    at_frame: at,
                    parameters: Self::parameters(gain),
                    ramp_frames: ramp,
                };
                match engine.command(s.id, command) {
                    Ok(()) => s.last = at,
                    Err(AudioError::UnknownHandle) => {}
                    Err(e) => return Err(e),
                }
            }
            play.intensity = intensity;
        }
        Ok(())
    }

    fn rule(&self, from: Option<usize>, to: usize) -> TransitionDesc {
        let from = from.map_or("", |f| self.desc.segments[f].name.as_str());
        let to_name = &self.desc.segments[to].name;
        self.desc
            .transitions
            .iter()
            .filter(|t| t.to == *to_name)
            .find(|t| t.from == from)
            .or_else(|| {
                self.desc
                    .transitions
                    .iter()
                    .find(|t| t.to == *to_name && t.from.is_empty())
            })
            .cloned()
            .unwrap_or(TransitionDesc {
                to: to_name.clone(),
                ..TransitionDesc::default()
            })
    }

    /// The first `sync` boundary at or after now in the playing segment's grid.
    fn boundary(&self, now: u64, sync: MusicSync) -> u64 {
        match self.current.as_ref().filter(|c| c.start <= now) {
            Some(c) => {
                c.start
                    + self.grids[c.segment]
                        .next((now - c.start) as f64, sync)
                        .round() as u64
            }
            None => self.current.as_ref().map_or(now, |c| c.start.max(now)),
        }
    }

    /// Requests segment `to`: it lands on the rule's boundary (bar by default), crossfading
    /// over the rule's fade, through its bridge if any. Returns the landing frame.
    pub fn transition(
        &mut self,
        engine: &mut AudioEngine,
        loader: &mut dyn StemLoader,
        to: &str,
    ) -> Result<u64> {
        let target = self.segment(Some(to))?;
        self.promote(engine)?;
        let now = engine.next_frame();
        let rule = self.rule(self.current.as_ref().map(|c| c.segment), target);
        let at = self.boundary(now, rule.sync);
        let fade = (rule.fade * self.rate as f32) as u32;
        if let Some(next) = self.next.take() {
            for s in next.stems {
                let _ = engine.command(
                    s.id,
                    VoiceCommand::Stop {
                        at_frame: now.max(s.last),
                        fade_frames: 0,
                    },
                );
            }
        }
        if let Some(current) = self.current.take() {
            self.previous = Some((current.segment, current.start));
            for s in current.stems {
                let _ = engine.command(
                    s.id,
                    VoiceCommand::Stop {
                        at_frame: at.max(s.last),
                        fade_frames: fade,
                    },
                );
            }
        }
        let first = if rule.bridge.is_empty() {
            self.then = None;
            target
        } else {
            self.then = Some(target);
            self.segment(Some(&rule.bridge))?
        };
        self.current = Some(self.schedule(engine, loader, first, at, fade)?);
        Ok(at)
    }

    /// Plays a stinger on its grid boundary. Returns its start frame.
    pub fn stinger(
        &mut self,
        engine: &mut AudioEngine,
        loader: &mut dyn StemLoader,
        name: &str,
    ) -> Result<u64> {
        let stinger = self
            .desc
            .stingers
            .get(name)
            .cloned()
            .ok_or(AudioError::Invalid("unknown music stinger"))?;
        self.promote(engine)?;
        let at = self.boundary(engine.next_frame(), stinger.sync);
        let playback = Playback {
            at_frame: at,
            spatial: false,
            priority: 250,
            parameters: Self::parameters(linear(stinger.gain_db)),
            ..Playback::default()
        };
        let clip = stinger
            .clip
            .as_ref()
            .ok_or(AudioError::Invalid("music stinger without a clip"))?;
        engine.install(self.bus, loader.prepare(&clip.0, playback)?)?;
        Ok(at)
    }

    /// Fades every playing stem toward its layer weight at `intensity` (0..1) over
    /// `layer_fade`; queued segments pick it up when they start.
    pub fn set_intensity(&mut self, engine: &mut AudioEngine, intensity: f32) -> Result<()> {
        if !intensity.is_finite() {
            return Err(AudioError::Invalid("music intensity"));
        }
        self.intensity = intensity.clamp(0.0, 1.0);
        self.promote(engine)
    }

    /// The segment and bar/beat audible at `frame`.
    pub fn position(&self, frame: u64) -> Option<MusicPosition> {
        let (segment, start) = match &self.current {
            Some(c) if c.start <= frame => (c.segment, c.start),
            _ => self.previous?,
        };
        let segment_frame = frame.checked_sub(start)?;
        Some(MusicPosition {
            segment: self.desc.segments[segment].name.clone(),
            at: self.grids[segment].position(segment_frame as f64),
        })
    }
}
