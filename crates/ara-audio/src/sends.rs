//! Per-voice auxiliary sends, such as the listener's room reverb.
//!
//! Spatial voices on a routed bus add `mono × AcousticPath::reverb_send` into a fixed
//! accumulator. Kira renders every bus before any send track within a quantum, so the
//! aux return (an effect on a send track) reads this quantum's sum: no added latency.
//! The accumulator is a fixed array of relaxed atomics: one renderer thread writes and
//! reads it sequentially, and nothing allocates or locks.

use crate::{QUANTUM, effects::Meter};
use kira::{Frame, effect::Effect, info::Info};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

/// Graph-local aux return handle, returned by `AudioEngine::add_aux`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AuxId(pub(crate) usize);

/// Allocation-free processor for an aux return. Construction may allocate; `process` must not.
pub trait AuxEffect: Send {
    /// Renders one quantum. `input` is the summed mono send of every routed voice;
    /// overwrite `out` (same length) with the stereo return.
    fn process(&mut self, input: &[f32], out: &mut [Frame], sample_rate: u32);
}

/// Mono send accumulator shared by routed voices and one aux return.
pub(crate) struct AuxInput {
    samples: [AtomicU32; QUANTUM],
}

impl AuxInput {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            samples: std::array::from_fn(|_| AtomicU32::new(0)),
        })
    }

    /// Adds `mono × gain`, with gain ramped linearly from `from` to `to` over the block.
    pub(crate) fn add(&self, mono: &[f32], from: f32, to: f32) {
        let step = (to - from) / mono.len().max(1) as f32;
        for (i, (slot, sample)) in self.samples.iter().zip(mono).enumerate() {
            let gain = from + step * (i + 1) as f32;
            let sum = f32::from_bits(slot.load(Ordering::Relaxed)) + sample * gain;
            slot.store(sum.to_bits(), Ordering::Relaxed);
        }
    }

    /// Moves the accumulated sum into `out` and clears it for the next quantum.
    fn take(&self, out: &mut [f32]) {
        for (slot, sample) in self.samples.iter().zip(out) {
            *sample = f32::from_bits(slot.swap(0, Ordering::Relaxed));
        }
    }
}

/// A voice's aux route: the shared accumulator plus its bus's effective linear gain,
/// so muting or fading a bus also silences its reverb send.
#[derive(Clone)]
pub(crate) struct AuxRoute {
    pub input: Arc<AuxInput>,
    pub bus_gain: Arc<AtomicU32>,
}

impl AuxRoute {
    pub(crate) fn scale(&self) -> f32 {
        f32::from_bits(self.bus_gain.load(Ordering::Relaxed))
    }
}

/// Kira effect on the aux send track: drains the accumulator and runs the aux processor.
pub(crate) struct AuxReturn {
    pub input: Arc<AuxInput>,
    pub effect: Box<dyn AuxEffect>,
    pub meter: Meter,
    pub sample_rate: u32,
    pub mono: [f32; QUANTUM],
    pub wet: [Frame; QUANTUM],
}

impl Effect for AuxReturn {
    fn init(&mut self, sample_rate: u32, _: usize) {
        self.sample_rate = sample_rate;
    }
    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }
    fn process(&mut self, input: &mut [Frame], _: f64, _: &Info) {
        let n = input.len().min(QUANTUM);
        self.input.take(&mut self.mono[..n]);
        self.effect
            .process(&self.mono[..n], &mut self.wet[..n], self.sample_rate);
        for (frame, wet) in input.iter_mut().zip(&self.wet[..n]) {
            *frame += *wet;
        }
        self.meter.observe(&input[..n]);
    }
}
