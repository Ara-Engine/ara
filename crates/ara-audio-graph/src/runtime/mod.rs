//! Running a compiled program as an `ara_audio::SignalSource`.
//!
//! `Program::instantiate` allocates a voice's arena and node state on the loader and
//! returns the source plus a [`GraphHandle`]. Gameplay sets parameters and fires
//! triggers through the handle with output-clock frames: triggers land on their exact
//! sample, parameter changes ramp across the block they land in. The audio thread only
//! drains a bounded ring.

mod ops;

use crate::{
    ast::{Output, Type},
    compile::{Program, SlotKind},
};
use ara_audio::{
    AudioError, Frame, QUANTUM, Result, SignalSource,
    capture::{GraphActivity, NodeActivity},
};
use ops::{Buf, Ctx, State};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

/// Commands queued per voice before the audio thread drains them.
pub const CONTROL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy)]
enum Control {
    Set { at: u64, slot: u16, value: f32 },
    Trigger { at: u64, slot: u16 },
}

impl Control {
    fn at(self) -> u64 {
        match self {
            Control::Set { at, .. } | Control::Trigger { at, .. } => at,
        }
    }
}

#[derive(Default)]
struct NodeStats {
    blocks: AtomicU64,
    active: AtomicU64,
    peak: AtomicU32,
    triggers: AtomicU64,
}

/// Lock-free per-node activity counters, written by the voice and read by tools.
struct Activity {
    nodes: Box<[NodeStats]>,
}

/// Control-path handle for one graph voice.
pub struct GraphHandle {
    program: Arc<Program>,
    producer: Producer<Control>,
    activity: Arc<Activity>,
    last: u64,
}

/// The audio-thread half: a `SignalSource` for `PreparedVoice::procedural`.
pub struct GraphVoice {
    program: Arc<Program>,
    states: Box<[State]>,
    arena: Box<[Buf]>,
    values: Box<[f32]>,
    targets: Box<[f32]>,
    controls: Consumer<Control>,
    pending: Option<Control>,
    activity: Arc<Activity>,
    last_clip: Box<[f32]>,
    ended: bool,
}

impl Program {
    /// Allocates one voice. `seed` makes its random nodes reproducible (derive it from
    /// the deterministic gameplay RNG, e.g. entity and play count).
    pub fn instantiate(self: &Arc<Self>, seed: u64) -> (GraphVoice, GraphHandle) {
        let mut arena = vec![[0.0_f32; QUANTUM]; self.buffers].into_boxed_slice();
        for &(buffer, value) in &self.constants {
            arena[usize::from(buffer)] = [value; QUANTUM];
        }
        let values: Box<[f32]> = self
            .slots
            .iter()
            .map(|s| match s.kind {
                SlotKind::Value { default, .. } => default,
                SlotKind::Clip { slot } => f32::from(slot),
                SlotKind::Trigger => 0.0,
            })
            .collect();
        let states = self
            .instrs
            .iter()
            .enumerate()
            .map(|(i, instr)| {
                let node_seed = seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                State::new(instr, node_seed, self.sample_rate)
            })
            .collect();
        let activity = Arc::new(Activity {
            nodes: (0..self.instrs.len())
                .map(|_| NodeStats::default())
                .collect(),
        });
        let (producer, controls) = RingBuffer::new(CONTROL_CAPACITY);
        let voice = GraphVoice {
            program: Arc::clone(self),
            states,
            arena,
            targets: values.clone(),
            values,
            controls,
            pending: None,
            activity: Arc::clone(&activity),
            last_clip: vec![f32::NAN; self.instrs.len()].into_boxed_slice(),
            ended: false,
        };
        let handle = GraphHandle {
            program: Arc::clone(self),
            producer,
            activity,
            last: 0,
        };
        (voice, handle)
    }
}

impl GraphHandle {
    fn slot(&self, name: &str, trigger: bool) -> Result<(u16, SlotKind)> {
        let (i, slot) = self
            .program
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .ok_or(AudioError::Invalid("unknown graph parameter or trigger"))?;
        if trigger != (slot.kind == SlotKind::Trigger) {
            return Err(AudioError::Invalid("parameter/trigger kind mismatch"));
        }
        Ok((i as u16, slot.kind))
    }

    fn push(&mut self, control: Control) -> Result<()> {
        if control.at() < self.last {
            return Err(AudioError::Invalid("graph controls must be in frame order"));
        }
        self.producer
            .push(control)
            .map_err(|_| AudioError::Capacity("graph control queue"))?;
        self.last = control.at();
        Ok(())
    }

    /// Sets a value parameter from `at_frame` (clamped to its declared range). Schedule at
    /// `AudioEngine::next_frame()` or later for deterministic offline renders.
    pub fn set(&mut self, name: &str, value: f32, at_frame: u64) -> Result<()> {
        if !value.is_finite() {
            return Err(AudioError::Invalid("graph parameter must be finite"));
        }
        let (slot, kind) = self.slot(name, false)?;
        let SlotKind::Value { range, .. } = kind else {
            return Err(AudioError::Invalid(
                "clip parameters are set at instantiation",
            ));
        };
        let value = range.map_or(value, |[lo, hi]| value.clamp(lo, hi));
        self.push(Control::Set {
            at: at_frame,
            slot,
            value,
        })
    }

    /// Fires a trigger on the exact output frame `at_frame`.
    pub fn trigger(&mut self, name: &str, at_frame: u64) -> Result<()> {
        let (slot, _) = self.slot(name, true)?;
        self.push(Control::Trigger { at: at_frame, slot })
    }

    /// Per-node activity so far, for `capture::Analysis::graphs`.
    pub fn activity(&self, instance: &str) -> GraphActivity {
        GraphActivity {
            graph: self.program.name.clone(),
            instance: instance.to_owned(),
            nodes: self
                .program
                .nodes
                .iter()
                .zip(self.activity.nodes.iter())
                .map(|((name, op), s)| NodeActivity {
                    node: name.clone(),
                    op: (*op).to_owned(),
                    blocks: s.blocks.load(Ordering::Relaxed),
                    active_blocks: s.active.load(Ordering::Relaxed),
                    peak: f32::from_bits(s.peak.load(Ordering::Relaxed)),
                    triggers: s.triggers.load(Ordering::Relaxed),
                })
                .collect(),
        }
    }

    /// The compiled program.
    pub fn program(&self) -> &Arc<Program> {
        &self.program
    }
}

impl GraphVoice {
    /// Applies queued controls that land in `[frame, frame + n)`: parameter targets and
    /// trigger pulses at their sample offsets.
    fn controls(&mut self, frame: u64, n: usize) {
        let end = frame + n as u64;
        for slot in &self.program.slots {
            if slot.kind == SlotKind::Trigger {
                self.arena[usize::from(slot.buffer)][..n].fill(0.0);
            }
        }
        loop {
            if self.pending.is_none() {
                self.pending = self.controls.pop().ok();
            }
            let Some(control) = self.pending else { break };
            if control.at() >= end {
                break;
            }
            self.pending = None;
            match control {
                Control::Set { slot, value, .. } => self.targets[usize::from(slot)] = value,
                Control::Trigger { at, slot } => {
                    let offset = at.saturating_sub(frame) as usize;
                    let buffer = self.program.slots[usize::from(slot)].buffer;
                    self.arena[usize::from(buffer)][offset.min(n - 1)] = 1.0;
                }
            }
        }
        for (i, slot) in self.program.slots.iter().enumerate() {
            if let SlotKind::Value { .. } | SlotKind::Clip { .. } = slot.kind {
                let (from, to) = (self.values[i], self.targets[i]);
                let buffer = &mut self.arena[usize::from(slot.buffer)];
                if from == to {
                    buffer[..n].fill(to);
                } else {
                    let step = (to - from) / n as f32;
                    for (k, v) in buffer[..n].iter_mut().enumerate() {
                        *v = from + step * (k + 1) as f32;
                    }
                    self.values[i] = to;
                }
            }
        }
    }
}

impl SignalSource for GraphVoice {
    fn sample_rate(&self) -> u32 {
        self.program.sample_rate
    }

    fn render(&mut self, out: &mut [Frame], frame: u64) {
        let n = out.len().min(QUANTUM);
        if self.ended || n == 0 {
            out.fill(Frame::ZERO);
            return;
        }
        self.controls(frame, n);
        let program = &self.program;
        let ctx = Ctx {
            rate: program.sample_rate as f32,
            clips: &program.clips,
            n,
        };
        for (((instr, state), stats), last) in program
            .instrs
            .iter()
            .zip(self.states.iter_mut())
            .zip(self.activity.nodes.iter())
            .zip(self.last_clip.iter_mut())
        {
            let start = usize::from(instr.out_start);
            let (inputs, rest) = self.arena.split_at_mut(start);
            let outputs = &mut rest[..usize::from(instr.outputs)];
            ops::run(instr, state, inputs, outputs, &ctx);
            // inf/NaN (pow of 0, huge gains) never reaches downstream filter state.
            for out in outputs.iter_mut() {
                for v in &mut out[..n] {
                    if !v.is_finite() {
                        *v = 0.0;
                    }
                }
            }
            let first = &outputs[0][..n];
            stats.blocks.fetch_add(1, Ordering::Relaxed);
            let active = match instr.first {
                Type::Audio => {
                    let peak = first.iter().fold(0.0_f32, |p, v| p.max(v.abs()));
                    stats.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
                    peak > 1e-4
                }
                Type::Trigger => {
                    let pulses = first.iter().filter(|v| **v > 0.5).count() as u64;
                    stats.triggers.fetch_add(pulses, Ordering::Relaxed);
                    pulses > 0
                }
                // Clip selectors are active when they switch clips.
                Type::Clip => {
                    let changed = first.iter().any(|v| *v != *last);
                    *last = first[n - 1];
                    changed
                }
            };
            if active {
                stats.active.fetch_add(1, Ordering::Relaxed);
            }
        }
        let read = |b: u16| &self.arena[usize::from(b)];
        match (
            program.outputs.get(&Output::Out),
            program.outputs.get(&Output::Left),
            program.outputs.get(&Output::Right),
        ) {
            (Some(&mono), _, _) => {
                for (f, v) in out.iter_mut().zip(&read(mono)[..n]) {
                    *f = Frame::new(*v, *v);
                }
            }
            (None, Some(&l), Some(&r)) => {
                for ((f, l), r) in out.iter_mut().zip(&read(l)[..n]).zip(&read(r)[..n]) {
                    *f = Frame::new(*l, *r);
                }
            }
            _ => out.fill(Frame::ZERO),
        }
        if let Some(&done) = program.outputs.get(&Output::Done)
            && let Some(at) = read(done)[..n].iter().position(|v| *v > 0.5)
        {
            out[at..].fill(Frame::ZERO);
            self.ended = true;
        }
    }

    fn ended(&self) -> bool {
        self.ended
    }
}
