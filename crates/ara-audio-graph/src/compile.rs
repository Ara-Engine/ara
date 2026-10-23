//! Lowers a checked graph to a flat program: one instruction per node in dependency
//! order over a buffer arena. Buffer 0 is silence; constants, parameters and clip slots
//! come next; each instruction's outputs are allocated after all of its inputs, so the
//! runtime borrows inputs and outputs from one arena with a single `split_at_mut`.

use crate::{
    ast::{Output, ParamKind, Type, Value},
    check::Checked,
    codes,
    library::Op,
};
use ara_audio::Clip;
use ara_diag::Diagnostic;
use std::{collections::BTreeMap, sync::Arc};

/// Resolves clip references (`@sfx/step.wav`) to resident clips on a loader thread.
pub trait ClipSource {
    /// Loads or shares the clip for `reference`.
    fn clip(&mut self, reference: &str) -> ara_audio::Result<Clip>;
}

impl<F: FnMut(&str) -> ara_audio::Result<Clip>> ClipSource for F {
    fn clip(&mut self, reference: &str) -> ara_audio::Result<Clip> {
        self(reference)
    }
}

/// Index of the always-silent buffer, used for unconnected optional inputs.
pub(crate) const ZERO: u16 = 0;

/// What a parameter slot carries at runtime.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SlotKind {
    Value {
        default: f32,
        range: Option<[f32; 2]>,
    },
    Trigger,
    Clip {
        slot: u16,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub name: String,
    pub buffer: u16,
    pub kind: SlotKind,
}

#[derive(Debug, Clone)]
pub(crate) struct Instr {
    pub op: Op,
    /// Input buffers: fixed inputs in spec order, then variadic items.
    pub inputs: Vec<u16>,
    pub out_start: u16,
    pub outputs: u16,
    /// Literal-only values (`seed`, delay `max`), in spec input order where present.
    pub seed: f32,
    pub capacity: f32,
    /// The `sample` trigger input is unconnected: start at the first frame.
    pub auto_start: bool,
    /// Type of the first output: activity counts audio level, trigger pulses or clip switches.
    pub first: Type,
}

/// A compiled, immutable program. Share it (`Arc`) between every voice of the sound.
#[derive(Debug)]
pub struct Program {
    pub(crate) name: String,
    pub(crate) sample_rate: u32,
    pub(crate) buffers: usize,
    pub(crate) constants: Vec<(u16, f32)>,
    pub(crate) slots: Vec<Slot>,
    pub(crate) clips: Vec<Clip>,
    pub(crate) instrs: Vec<Instr>,
    pub(crate) nodes: Vec<(String, &'static str)>,
    pub(crate) outputs: BTreeMap<Output, u16>,
    pub(crate) cost: u32,
}

impl Program {
    /// Graph name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Sample rate the program was compiled for.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// True for `left`/`right` output (non-spatial stereo voices).
    pub fn stereo(&self) -> bool {
        self.outputs.contains_key(&Output::Left)
    }
    /// Library cost estimate per 128-frame block, in nanoseconds.
    pub fn estimated_cost(&self) -> u32 {
        self.cost
    }
    /// Instruction count.
    pub fn len(&self) -> usize {
        self.instrs.len()
    }
    /// True for a program without operations.
    pub fn is_empty(&self) -> bool {
        self.instrs.is_empty()
    }
    /// Arena bytes each voice allocates at instantiation.
    pub fn arena_bytes(&self) -> usize {
        self.buffers * ara_audio::QUANTUM * 4
    }
    /// Gameplay parameter and trigger names, in declaration order.
    pub fn controls(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().map(|s| s.name.as_str())
    }
}

fn err(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::GRAPH, message)
}

struct Lower<'a> {
    buffers: usize,
    constants: BTreeMap<u32, u16>,
    constant_list: Vec<(u16, f32)>,
    clips: Vec<Clip>,
    clip_slots: BTreeMap<String, u16>,
    source: &'a mut dyn ClipSource,
}

impl Lower<'_> {
    fn allocate(&mut self, count: usize) -> Result<u16, Diagnostic> {
        let start = u16::try_from(self.buffers).map_err(|_| err("graph needs too many buffers"))?;
        self.buffers += count;
        if self.buffers > usize::from(u16::MAX) {
            return Err(err("graph needs too many buffers"));
        }
        Ok(start)
    }
    fn constant(&mut self, value: f32) -> Result<u16, Diagnostic> {
        if let Some(&b) = self.constants.get(&value.to_bits()) {
            return Ok(b);
        }
        let b = self.allocate(1)?;
        self.constants.insert(value.to_bits(), b);
        self.constant_list.push((b, value));
        Ok(b)
    }
    fn clip(&mut self, reference: &str) -> Result<u16, Diagnostic> {
        if let Some(&slot) = self.clip_slots.get(reference) {
            return Ok(slot);
        }
        let clip = self
            .source
            .clip(reference)
            .map_err(|e| err(format!("clip `{reference}`: {e}")))?;
        let slot = u16::try_from(self.clips.len()).map_err(|_| err("too many clips"))?;
        self.clips.push(clip);
        self.clip_slots.insert(reference.to_owned(), slot);
        Ok(slot)
    }
}

/// Compiles a checked graph for `sample_rate`, resolving every clip through `clips`.
/// Runs on a loader: allocates, may decode. The result renders without allocating.
pub fn compile(
    checked: &Checked,
    sample_rate: u32,
    clips: &mut dyn ClipSource,
) -> Result<Arc<Program>, Vec<Diagnostic>> {
    if !(8000..=192000).contains(&sample_rate) {
        return Err(vec![err("sample rate")]);
    }
    let graph = &checked.graph;
    let mut lower = Lower {
        buffers: 1,
        constants: BTreeMap::new(),
        constant_list: Vec::new(),
        clips: Vec::new(),
        clip_slots: BTreeMap::new(),
        source: clips,
    };
    let one = |r: Result<u16, Diagnostic>| r.map_err(|d| vec![d]);
    let mut params: Vec<_> = graph.params.iter().collect();
    params.sort_by_key(|(_, p)| p.order);
    let mut buffer_of: BTreeMap<&str, u16> = BTreeMap::new();
    let mut slots = Vec::with_capacity(params.len());
    for (id, p) in params {
        let buffer = one(lower.allocate(1))?;
        let kind = match &p.kind {
            ParamKind::Value { default, range } => SlotKind::Value {
                default: *default,
                range: *range,
            },
            ParamKind::Trigger => SlotKind::Trigger,
            ParamKind::Clip { default } => SlotKind::Clip {
                slot: one(lower.clip(default))?,
            },
        };
        buffer_of.insert(id, buffer);
        slots.push(Slot {
            name: p.name.clone(),
            buffer,
            kind,
        });
    }
    let mut instrs = Vec::with_capacity(checked.order.len());
    let mut nodes = Vec::with_capacity(checked.order.len());
    let mut node_out: BTreeMap<&str, u16> = BTreeMap::new();
    let resolve = |lower: &mut Lower<'_>,
                   node_out: &BTreeMap<&str, u16>,
                   buffer_of: &BTreeMap<&str, u16>,
                   v: &Value|
     -> Result<u16, Diagnostic> {
        match v {
            Value::Number { value } => lower.constant(*value),
            Value::Clip { reference } => {
                let slot = lower.clip(reference)?;
                lower.constant(f32::from(slot))
            }
            Value::Ref { node, port } => {
                if let Some(&b) = buffer_of.get(node.as_str()) {
                    return Ok(b);
                }
                let start = *node_out
                    .get(node.as_str())
                    .ok_or_else(|| err(format!("unresolved reference `{node}`")))?;
                let spec = graph.nodes[node].op.spec();
                let index = port.as_deref().and_then(|p| spec.output(p)).unwrap_or(0);
                Ok(start + index as u16)
            }
        }
    };
    let mut cost = 0;
    for id in &checked.order {
        let node = &graph.nodes[id];
        let spec = node.op.spec();
        let mut inputs = Vec::with_capacity(spec.inputs.len() + 4);
        let mut seed = 0.0;
        let mut capacity = 1.0;
        let mut auto_start = false;
        for input in spec.inputs {
            let buffer = match node.args.get(input.name) {
                Some(v) => {
                    if let Value::Number { value } = v {
                        match input.name {
                            "seed" => seed = *value,
                            "max" => capacity = *value,
                            _ => {}
                        }
                    }
                    one(resolve(&mut lower, &node_out, &buffer_of, v))?
                }
                None => match input.default {
                    Some(d) if !d.is_nan() => {
                        if input.name == "max" {
                            capacity = d;
                        }
                        one(lower.constant(d))?
                    }
                    _ => {
                        auto_start |= node.op == Op::Sample && input.name == "trigger";
                        ZERO
                    }
                },
            };
            inputs.push(buffer);
        }
        for i in 0.. {
            let Some(v) = node.args.get(&format!("in{i}")) else {
                break;
            };
            inputs.push(one(resolve(&mut lower, &node_out, &buffer_of, v))?);
        }
        let outputs = spec.outputs.len() as u16;
        let out_start = one(lower.allocate(usize::from(outputs)))?;
        node_out.insert(id, out_start);
        let types = &checked.types[id];
        instrs.push(Instr {
            op: node.op,
            inputs,
            out_start,
            outputs,
            seed,
            capacity: capacity.clamp(0.001, 10.0),
            auto_start,
            first: types.first().copied().unwrap_or(Type::Audio),
        });
        nodes.push((node.name.clone(), spec.name));
        cost += spec.cost;
    }
    let mut outputs = BTreeMap::new();
    for (field, value) in &graph.outputs {
        outputs.insert(
            *field,
            one(resolve(&mut lower, &node_out, &buffer_of, value))?,
        );
    }
    Ok(Arc::new(Program {
        name: graph.name.clone(),
        sample_rate,
        buffers: lower.buffers,
        constants: lower.constant_list,
        slots,
        clips: lower.clips,
        instrs,
        nodes,
        outputs,
        cost,
    }))
}
