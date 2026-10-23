//! Builds a `MixerDesc` into an `AudioEngine` and switches mix states.

use crate::desc::{BusDesc, MixerDesc};
use ara_audio::{
    AudioEngine, AuxId, BusId, Snapshot,
    diagnose::codes,
    effects::{Ducking, TrackBuilder},
};
use ara_diag::Diagnostic;
use std::collections::BTreeMap;

/// The engine-side mixer: bus handles by name and the active state.
pub struct Mixer {
    buses: BTreeMap<String, BusId>,
    base: BTreeMap<String, f32>,
    desc: MixerDesc,
    state: Option<String>,
}

/// Linear gain for dB; -96 dB and below is silence.
pub fn linear(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f32.powf(db / 20.0).min(16.0)
    }
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::MIXER, message)
}

fn base_gain(bus: &BusDesc) -> f32 {
    if bus.mute { 0.0 } else { linear(bus.gain_db) }
}

impl Mixer {
    /// Creates every bus (parents and duck sources first), applies limits, loudness
    /// targets and reverb routes, then the initial state. `room` is the room-reverb aux.
    /// Control path: call while configuring the engine, before playback.
    pub fn build(
        engine: &mut AudioEngine,
        desc: MixerDesc,
        room: Option<AuxId>,
    ) -> Result<Self, Diagnostic> {
        let fail = |what: &str, e: ara_audio::AudioError| error(format!("{what}: {e}"));
        let mut buses = BTreeMap::from([("master".to_owned(), BusId::MASTER)]);
        let mut base = BTreeMap::new();
        for bus in &desc.buses {
            let id = if bus.name == "master" {
                BusId::MASTER
            } else {
                let mut builder = TrackBuilder::new();
                if let Some(duck) = &bus.duck {
                    let source = engine
                        .bus_meter(buses[&duck.by])
                        .map_err(|e| fail("duck source", e))?
                        .clone();
                    let effect = Ducking {
                        source,
                        threshold: linear(duck.threshold_db),
                        gain: linear(duck.amount_db),
                        attack: f64::from(duck.attack),
                        release: f64::from(duck.release),
                    }
                    .build()
                    .map_err(|e| fail("ducking", e))?;
                    builder.add_built_effect(effect);
                }
                let parent = if bus.output.is_empty() {
                    "master"
                } else {
                    &bus.output
                };
                engine
                    .add_bus(bus.name.clone(), buses[parent], builder)
                    .map_err(|e| fail(&bus.name, e))?
            };
            buses.insert(bus.name.clone(), id);
            base.insert(bus.name.clone(), base_gain(bus));
            let limit = (bus.voice_limit > 0).then_some(bus.voice_limit as usize);
            engine
                .set_voice_limit(id, limit)
                .map_err(|e| fail(&bus.name, e))?;
            engine
                .set_loudness_target(id, bus.loudness)
                .map_err(|e| fail(&bus.name, e))?;
            if bus.reverb {
                engine.route_aux(id, room).map_err(|e| fail(&bus.name, e))?;
            }
        }
        base.entry("master".into()).or_insert(1.0);
        let initial =
            (!desc.settings.initial_state.is_empty()).then(|| desc.settings.initial_state.clone());
        let mut mixer = Self {
            buses,
            base,
            desc,
            state: None,
        };
        mixer.set_state(engine, initial.as_deref(), Some(0.0))?;
        Ok(mixer)
    }

    /// Resolves an `AudioSource.bus` name.
    pub fn bus(&self, name: &str) -> Option<BusId> {
        self.buses.get(name).copied()
    }

    /// The active state; None is the base mix.
    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }

    /// Target linear gain of every bus in `state` (None: the base mix).
    pub fn gains(&self, state: Option<&str>) -> Result<BTreeMap<String, f32>, Diagnostic> {
        let offsets = match state {
            None => None,
            Some(name) => Some(
                self.desc
                    .states
                    .get(name)
                    .ok_or_else(|| error(format!("unknown mix state `{name}`")))?,
            ),
        };
        Ok(self
            .base
            .iter()
            .map(|(bus, gain)| {
                let offset = offsets
                    .and_then(|s| s.buses.get(bus))
                    .copied()
                    .unwrap_or(0.0);
                (bus.clone(), (gain * linear(offset)).min(16.0))
            })
            .collect())
    }

    /// Transitions to `state` (None: base mix) over the state's `fade`, or `fade_override`
    /// seconds. Bus gains tween in the engine; voices keep their own sample-accurate gains.
    pub fn set_state(
        &mut self,
        engine: &mut AudioEngine,
        state: Option<&str>,
        fade_override: Option<f32>,
    ) -> Result<(), Diagnostic> {
        let gains = self.gains(state)?;
        let fade = fade_override
            .or_else(|| state.and_then(|s| self.desc.states.get(s)).map(|s| s.fade))
            .unwrap_or(1.0)
            .max(0.0);
        let snapshot = Snapshot {
            name: state.unwrap_or("base").to_owned(),
            gains: gains
                .iter()
                .map(|(bus, gain)| (self.buses[bus], *gain))
                .collect(),
        };
        let frames = (fade * engine.sample_rate() as f32) as u32;
        engine
            .apply_snapshot(&snapshot, frames)
            .map_err(|e| error(format!("state `{}`: {e}", snapshot.name)))?;
        self.state = state.map(str::to_owned);
        Ok(())
    }
}
