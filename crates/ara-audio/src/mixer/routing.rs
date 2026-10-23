//! Category policy and aux routing: per-bus voice limits, loudness targets, room-reverb
//! returns, bus gains and voice audibility state. All methods run on the control path.

use super::{AudioEngine, Bus, BusId, VoiceId};
use crate::{
    AudioError, Frame, QUANTUM, Result,
    effects::{Meter, SendTrackBuilder},
    sends::{AuxEffect, AuxId, AuxInput, AuxReturn, AuxRoute},
    voice::cull,
};
use kira::{Tween, track::SendTrackHandle};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

pub(crate) struct AuxSlot {
    pub name: String,
    pub input: Arc<AuxInput>,
    pub handle: SendTrackHandle,
    pub meter: Meter,
}

/// Where a voice stands on the real-voice budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VoiceStatus {
    /// Rendering as a real voice.
    Playing,
    /// Virtualized because the engine's real-voice limit is full of higher priorities.
    VirtualGlobalLimit,
    /// Virtualized because its bus (or an ancestor bus) reached its category voice limit.
    VirtualBusLimit,
    /// Scheduled to start later, or finished this quantum.
    Idle,
}

/// Presentation state of one voice, for diagnostics and the `ara why` audio explainer.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceState {
    /// Bus the voice plays on.
    pub bus: BusId,
    /// Budget status at the last rendered quantum.
    pub status: VoiceStatus,
    /// Current voice gain (source gain × distance/cone attenuation).
    pub gain: f32,
    /// Loudness normalization trim applied at installation.
    pub trim: f32,
    /// Output frames rendered so far.
    pub position: u64,
}

/// `bus`, its parent, ..., master.
pub(crate) fn chain(buses: &[Bus], bus: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(Some(bus), |&b| buses[b].parent)
}

fn tween(frames: u32, rate: u32) -> Tween {
    Tween {
        duration: Duration::from_secs_f64(frames as f64 / rate as f64),
        ..Tween::default()
    }
}

fn decibels(gain: f32) -> f32 {
    if gain == 0.0 {
        kira::Decibels::SILENCE.0
    } else {
        20.0 * gain.log10()
    }
}

impl AudioEngine {
    pub(crate) fn chain(&self, bus: usize) -> impl Iterator<Item = usize> + '_ {
        chain(&self.buses, bus)
    }

    pub(crate) fn aux_route(&self, bus: usize) -> Option<AuxRoute> {
        let aux = self.chain(bus).find_map(|b| self.buses[b].aux)?;
        Some(AuxRoute {
            input: Arc::clone(&self.auxes[aux.0].input),
            bus_gain: Arc::clone(&self.buses[bus].effective),
        })
    }

    /// Republishes each bus's target gain product, read by voices' aux sends.
    pub(crate) fn refresh_effective_gains(&mut self) {
        for i in 0..self.buses.len() {
            let gain: f32 = self.chain(i).map(|b| self.buses[b].gain).product();
            self.buses[i]
                .effective
                .store(gain.to_bits(), Ordering::Relaxed);
        }
    }

    /// Adds an aux return (e.g. `ara-acoustics` room reverb). Its output joins the master
    /// mix after every bus; route buses to it with `route_aux`. Shares send capacity.
    pub fn add_aux(
        &mut self,
        name: impl Into<String>,
        effect: Box<dyn AuxEffect>,
    ) -> Result<AuxId> {
        let name = name.into();
        if self.sends.len() + self.auxes.len() >= self.config.buses {
            return Err(AudioError::Capacity("effect sends"));
        }
        if name.is_empty() || self.auxes.iter().any(|a| a.name == name) {
            return Err(AudioError::Invalid("unique aux name required"));
        }
        let input = AuxInput::new();
        let meter = Meter::default();
        let mut builder = SendTrackBuilder::new();
        builder.add_built_effect(Box::new(AuxReturn {
            input: Arc::clone(&input),
            effect,
            meter: meter.clone(),
            sample_rate: self.config.sample_rate,
            mono: [0.0; QUANTUM],
            wet: [Frame::ZERO; QUANTUM],
        }));
        let handle = self
            .manager
            .add_send_track(builder)
            .map_err(|_| AudioError::Capacity("effect sends"))?;
        self.auxes.push(AuxSlot {
            name,
            input,
            handle,
            meter,
        });
        Ok(AuxId(self.auxes.len() - 1))
    }
    /// Resolves an aux return by name.
    pub fn aux(&self, name: &str) -> Option<AuxId> {
        self.auxes.iter().position(|a| a.name == name).map(AuxId)
    }
    /// Return-level meter of an aux (its wet output).
    pub fn aux_meter(&self, aux: AuxId) -> Result<&Meter> {
        self.auxes
            .get(aux.0)
            .map(|a| &a.meter)
            .ok_or(AudioError::UnknownHandle)
    }
    /// Tweens an aux return's output gain.
    pub fn set_aux_gain(&mut self, aux: AuxId, gain: f32, frames: u32) -> Result<()> {
        if !gain.is_finite() || !(0.0..=16.0).contains(&gain) {
            return Err(AudioError::Invalid("aux gain"));
        }
        let rate = self.config.sample_rate;
        self.auxes
            .get_mut(aux.0)
            .ok_or(AudioError::UnknownHandle)?
            .handle
            .set_volume(decibels(gain), tween(frames, rate));
        Ok(())
    }
    /// Routes spatial voices installed later on `bus` (or its descendants without their own
    /// route) to `aux`, scaled per voice by `AcousticPath::reverb_send`. None removes the route.
    pub fn route_aux(&mut self, bus: BusId, aux: Option<AuxId>) -> Result<()> {
        if aux.is_some_and(|a| a.0 >= self.auxes.len()) {
            return Err(AudioError::UnknownHandle);
        }
        self.buses
            .get_mut(bus.0)
            .ok_or(AudioError::UnknownHandle)?
            .aux = aux;
        Ok(())
    }
    /// Caps real voices in a category bus and its descendants; lower priority (then
    /// quieter, then newer) voices virtualize. None removes the cap.
    pub fn set_voice_limit(&mut self, bus: BusId, limit: Option<usize>) -> Result<()> {
        if limit.is_some_and(|l| l > self.config.total_voices) {
            return Err(AudioError::Invalid("voice limit exceeds total voices"));
        }
        self.buses
            .get_mut(bus.0)
            .ok_or(AudioError::UnknownHandle)?
            .voice_limit = limit;
        Ok(())
    }
    /// Normalizes voices installed later on this bus (and descendants) to an integrated
    /// loudness in LUFS, using clip or stream metadata. None disables normalization.
    pub fn set_loudness_target(&mut self, bus: BusId, lufs: Option<f32>) -> Result<()> {
        if lufs.is_some_and(|l| !l.is_finite() || !(-70.0..=0.0).contains(&l)) {
            return Err(AudioError::Invalid(
                "loudness target must be in [-70, 0] LUFS",
            ));
        }
        self.buses
            .get_mut(bus.0)
            .ok_or(AudioError::UnknownHandle)?
            .loudness = lufs;
        Ok(())
    }
    /// Tweens one bus's linear gain in [0,16]; zero mutes the bus and its reverb sends.
    pub fn set_bus_gain(&mut self, bus: BusId, gain: f32, frames: u32) -> Result<()> {
        if !gain.is_finite() || !(0.0..=16.0).contains(&gain) {
            return Err(AudioError::Invalid("bus gain"));
        }
        let rate = self.config.sample_rate;
        let b = self.buses.get_mut(bus.0).ok_or(AudioError::UnknownHandle)?;
        b.gain = gain;
        b.handle.set_volume(decibels(gain), tween(frames, rate));
        self.refresh_effective_gains();
        Ok(())
    }
    /// Target linear gain of one bus.
    pub fn bus_gain(&self, bus: BusId) -> Option<f32> {
        self.buses.get(bus.0).map(|b| b.gain)
    }
    /// Product of target gains from `bus` to master; zero means some bus on the path is muted.
    pub fn effective_gain(&self, bus: BusId) -> Option<f32> {
        (bus.0 < self.buses.len()).then(|| self.chain(bus.0).map(|b| self.buses[b].gain).product())
    }
    /// The muted bus on `bus`'s path to master closest to `bus`, if any.
    pub fn muted_ancestor(&self, bus: BusId) -> Option<BusId> {
        if bus.0 >= self.buses.len() {
            return None;
        }
        self.chain(bus.0)
            .find(|&b| self.buses[b].gain == 0.0)
            .map(BusId)
    }
    /// Bus name, for diagnostics and capture reports.
    pub fn bus_name(&self, bus: BusId) -> Option<&str> {
        self.buses.get(bus.0).map(|b| b.name.as_str())
    }
    /// Every bus in creation order (parents before children), with its name.
    pub fn buses(&self) -> impl Iterator<Item = (BusId, &str)> {
        self.buses
            .iter()
            .enumerate()
            .map(|(i, b)| (BusId(i), b.name.as_str()))
    }
    /// Every aux return in creation order, with its name.
    pub fn auxes(&self) -> impl Iterator<Item = (AuxId, &str)> {
        self.auxes
            .iter()
            .enumerate()
            .map(|(i, a)| (AuxId(i), a.name.as_str()))
    }
    /// A live or virtual voice's state; None once finished and collected, or unknown.
    pub fn voice_state(&self, id: VoiceId) -> Option<VoiceState> {
        let v = self.voices.iter().find(|v| v.id == id)?;
        let shared = &v.control.shared;
        let status = match shared.cull.load(Ordering::Relaxed) {
            cull::ACTIVE => VoiceStatus::Playing,
            cull::GLOBAL_LIMIT => VoiceStatus::VirtualGlobalLimit,
            cull::BUS_LIMIT => VoiceStatus::VirtualBusLimit,
            _ => VoiceStatus::Idle,
        };
        Some(VoiceState {
            bus: BusId(v.bus),
            status,
            gain: f32::from_bits(shared.audibility.load(Ordering::Relaxed)),
            trim: v.trim,
            position: shared.position.load(Ordering::Relaxed),
        })
    }
}
