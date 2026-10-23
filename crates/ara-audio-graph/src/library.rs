//! The documented node library: every operation's typed inputs, outputs, defaults and
//! estimated cost. One table drives the DSL, the checker, the graph registry and the DSP.

use crate::ast::Type;
use ara_diag::Diagnostic;
use ara_graph::{Direction, NodeKind, NodeRegistry, Port};
use std::collections::BTreeMap;

/// Port type rule: concrete, or the op's generic item type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRule {
    /// Always this type.
    Is(Type),
    /// Bound by the first variadic item (`switch`, `random.pick`).
    Generic,
}

/// An input socket.
#[derive(Debug, Clone, Copy)]
pub struct InputSpec {
    /// Socket name (named arguments, graph port key).
    pub name: &'static str,
    /// Accepted type.
    pub ty: TypeRule,
    /// Default literal when omitted; None means the input is required, NaN means
    /// "optional and unconnected" (triggers/gates with special behaviour).
    pub default: Option<f32>,
    /// Units and meaning.
    pub doc: &'static str,
}

/// An output socket.
#[derive(Debug, Clone, Copy)]
pub struct OutputSpec {
    /// Socket name (`player.done`); the first output is the default.
    pub name: &'static str,
    /// Produced type.
    pub ty: TypeRule,
    /// Meaning.
    pub doc: &'static str,
}

/// Variadic trailing inputs (`in0`, `in1`, ...).
#[derive(Debug, Clone, Copy)]
pub struct Variadic {
    /// Item type.
    pub ty: TypeRule,
    /// Minimum items.
    pub min: usize,
    /// Maximum items.
    pub max: usize,
    /// Meaning of each item.
    pub doc: &'static str,
}

/// One operation's documentation and signature.
#[derive(Debug, Clone, Copy)]
pub struct OpSpec {
    /// DSL/registry name (`lowpass`, `random.pick`).
    pub name: &'static str,
    /// Agent- and tooltip-facing description.
    pub doc: &'static str,
    /// Fixed inputs, in positional order.
    pub inputs: &'static [InputSpec],
    /// Trailing variadic items, if any.
    pub variadic: Option<Variadic>,
    /// Outputs; the first is used when a reference names no port.
    pub outputs: &'static [OutputSpec],
    /// Estimated cost per 128-frame block in nanoseconds (measured, see AGENTS.md).
    pub cost: u32,
}

const A: TypeRule = TypeRule::Is(Type::Audio);
const T: TypeRule = TypeRule::Is(Type::Trigger);
const C: TypeRule = TypeRule::Is(Type::Clip);
/// Marks an optional input that may stay unconnected (no literal default).
pub const UNCONNECTED: f32 = f32::NAN;

const fn i(name: &'static str, ty: TypeRule, default: Option<f32>, doc: &'static str) -> InputSpec {
    InputSpec {
        name,
        ty,
        default,
        doc,
    }
}
const fn o(name: &'static str, ty: TypeRule, doc: &'static str) -> OutputSpec {
    OutputSpec { name, ty, doc }
}
const OUT: &[OutputSpec] = &[o("out", A, "Audio-rate result.")];
const SPEC_NONE: Option<Variadic> = None;

macro_rules! ops {
    ($($kind:ident = $name:literal, $doc:literal, [$($input:expr),*], $variadic:expr, $outputs:expr, $cost:literal;)*) => {
        /// Every node operation.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Op { $(#[doc = $doc] $kind,)* }

        impl Op {
            /// Every operation, in library order.
            pub const ALL: &'static [Op] = &[$(Op::$kind),*];

            /// The operation's documented signature.
            pub fn spec(self) -> &'static OpSpec {
                match self {
                    $(Op::$kind => {
                        const SPEC: OpSpec = OpSpec {
                            name: $name,
                            doc: $doc,
                            inputs: &[$($input),*],
                            variadic: $variadic,
                            outputs: $outputs,
                            cost: $cost,
                        };
                        &SPEC
                    })*
                }
            }
        }
    };
}

ops! {
    Sine = "sine", "Sine oscillator.", [i("freq", A, Some(440.0), "Frequency in Hz."), i("phase", A, Some(0.0), "Phase offset in cycles.")], SPEC_NONE, OUT, 900;
    Saw = "saw", "Band-limited (polyBLEP) sawtooth, -1..1.", [i("freq", A, Some(440.0), "Frequency in Hz.")], SPEC_NONE, OUT, 700;
    Square = "square", "Band-limited (polyBLEP) pulse, -1..1.", [i("freq", A, Some(440.0), "Frequency in Hz."), i("width", A, Some(0.5), "Pulse width, 0..1.")], SPEC_NONE, OUT, 900;
    Triangle = "triangle", "Triangle oscillator, -1..1.", [i("freq", A, Some(440.0), "Frequency in Hz.")], SPEC_NONE, OUT, 500;
    Phasor = "phasor", "Rising ramp 0..1 (LFO building block).", [i("freq", A, Some(1.0), "Frequency in Hz.")], SPEC_NONE, OUT, 300;
    White = "noise.white", "Seeded white noise, -1..1.", [i("seed", A, Some(0.0), "Integer seed (literal).")], SPEC_NONE, OUT, 400;
    Pink = "noise.pink", "Seeded pink (-3 dB/octave) noise.", [i("seed", A, Some(0.0), "Integer seed (literal).")], SPEC_NONE, OUT, 700;
    Brown = "noise.brown", "Seeded brown (-6 dB/octave) noise.", [i("seed", A, Some(0.0), "Integer seed (literal).")], SPEC_NONE, OUT, 500;
    Smooth = "noise.smooth", "Smoothly interpolated random curve, -1..1 (gusts, wobble).", [i("rate", A, Some(1.0), "New random targets per second."), i("seed", A, Some(0.0), "Integer seed (literal).")], SPEC_NONE, OUT, 500;
    Adsr = "adsr", "Attack/decay/sustain/release envelope driven by a gate, 0..1.", [i("gate", A, None, "Held above 0.5 while on."), i("attack", A, Some(0.01), "Seconds."), i("decay", A, Some(0.1), "Seconds."), i("sustain", A, Some(0.7), "Level 0..1."), i("release", A, Some(0.3), "Seconds.")], SPEC_NONE, &[o("out", A, "Envelope 0..1."), o("done", T, "Fires when the release reaches zero.")], 700;
    Ad = "ad", "One-shot attack/decay envelope started by a trigger, 0..1.", [i("trigger", T, None, "Restarts the envelope."), i("attack", A, Some(0.005), "Seconds."), i("decay", A, Some(0.2), "Seconds.")], SPEC_NONE, &[o("out", A, "Envelope 0..1."), o("done", T, "Fires when the decay reaches zero.")], 500;
    Lowpass = "lowpass", "State-variable low-pass filter (12 dB/octave).", [i("in", A, None, "Input signal."), i("cutoff", A, Some(1000.0), "Cutoff in Hz."), i("q", A, Some(0.707), "Resonance.")], SPEC_NONE, OUT, 1200;
    Highpass = "highpass", "State-variable high-pass filter (12 dB/octave).", [i("in", A, None, "Input signal."), i("cutoff", A, Some(1000.0), "Cutoff in Hz."), i("q", A, Some(0.707), "Resonance.")], SPEC_NONE, OUT, 1200;
    Bandpass = "bandpass", "State-variable band-pass filter with unity peak gain.", [i("in", A, None, "Input signal."), i("cutoff", A, Some(1000.0), "Center in Hz."), i("q", A, Some(1.0), "Bandwidth: higher is narrower.")], SPEC_NONE, OUT, 1200;
    Notch = "notch", "State-variable notch filter.", [i("in", A, None, "Input signal."), i("cutoff", A, Some(1000.0), "Center in Hz."), i("q", A, Some(1.0), "Bandwidth: higher is narrower.")], SPEC_NONE, OUT, 1200;
    Peak = "peak", "Biquad peaking EQ.", [i("in", A, None, "Input signal."), i("freq", A, Some(1000.0), "Center in Hz."), i("q", A, Some(1.0), "Bandwidth."), i("gain", A, Some(0.0), "Boost/cut in dB.")], SPEC_NONE, OUT, 900;
    LowShelf = "lowshelf", "Biquad low shelf.", [i("in", A, None, "Input signal."), i("freq", A, Some(200.0), "Corner in Hz."), i("gain", A, Some(0.0), "Boost/cut in dB.")], SPEC_NONE, OUT, 900;
    HighShelf = "highshelf", "Biquad high shelf.", [i("in", A, None, "Input signal."), i("freq", A, Some(4000.0), "Corner in Hz."), i("gain", A, Some(0.0), "Boost/cut in dB.")], SPEC_NONE, OUT, 900;
    OnePole = "onepole", "One-pole low-pass (6 dB/octave), cheap smoothing filter.", [i("in", A, None, "Input signal."), i("cutoff", A, Some(1000.0), "Cutoff in Hz.")], SPEC_NONE, OUT, 300;
    Sample = "sample", "Clip player with loop region; starts on its trigger (or at voice start when unconnected).", [i("clip", C, None, "Clip to play."), i("trigger", T, Some(UNCONNECTED), "Restarts playback; picks up the current clip."), i("rate", A, Some(1.0), "Playback rate (pitch)."), i("start", A, Some(0.0), "Start offset in seconds."), i("loopStart", A, Some(0.0), "Loop region start in seconds."), i("loopEnd", A, Some(0.0), "Loop region end in seconds; 0 means the clip end."), i("loop", A, Some(0.0), "Above 0.5 loops the region.")], SPEC_NONE, &[o("out", A, "Mono mix (l+r)/2."), o("left", A, "Left channel."), o("right", A, "Right channel."), o("done", T, "Fires when a non-looping play reaches the end.")], 1500;
    Granular = "granular", "Granular clip player: overlapping Hann-windowed grains (up to 32).", [i("clip", C, None, "Source clip."), i("position", A, Some(0.5), "Read position 0..1 of the clip."), i("size", A, Some(0.08), "Grain length in seconds."), i("density", A, Some(20.0), "Grains per second."), i("pitch", A, Some(1.0), "Grain playback rate."), i("spread", A, Some(0.05), "Random position jitter 0..1."), i("seed", A, Some(0.0), "Integer seed (literal).")], SPEC_NONE, OUT, 6000;
    Mix = "mix", "Sums 2-8 signals.", [], Some(Variadic { ty: A, min: 2, max: 8, doc: "Signals to sum." }), OUT, 200;
    Add = "add", "a + b.", [i("a", A, None, "Left operand."), i("b", A, Some(0.0), "Right operand.")], SPEC_NONE, OUT, 100;
    Sub = "sub", "a - b.", [i("a", A, None, "Left operand."), i("b", A, Some(0.0), "Right operand.")], SPEC_NONE, OUT, 100;
    Mul = "mul", "a × b (gain, ring modulation, envelopes).", [i("a", A, None, "Left operand."), i("b", A, Some(1.0), "Right operand.")], SPEC_NONE, OUT, 100;
    Div = "div", "a / b; zero divisors give zero.", [i("a", A, None, "Numerator."), i("b", A, Some(1.0), "Denominator.")], SPEC_NONE, OUT, 150;
    Min = "min", "Smaller of a and b.", [i("a", A, None, "First value."), i("b", A, None, "Second value.")], SPEC_NONE, OUT, 100;
    Max = "max", "Larger of a and b.", [i("a", A, None, "First value."), i("b", A, None, "Second value.")], SPEC_NONE, OUT, 100;
    Abs = "abs", "Absolute value.", [i("in", A, None, "Input signal.")], SPEC_NONE, OUT, 100;
    Clamp = "clamp", "Limits a signal to lo..hi.", [i("in", A, None, "Input signal."), i("lo", A, Some(0.0), "Lower bound."), i("hi", A, Some(1.0), "Upper bound.")], SPEC_NONE, OUT, 150;
    Lerp = "lerp", "Linear blend a→b by t (0..1).", [i("a", A, None, "Value at t = 0."), i("b", A, None, "Value at t = 1."), i("t", A, Some(0.5), "Blend amount.")], SPEC_NONE, OUT, 150;
    Map = "map", "Maps inLo..inHi to outLo..outHi, clamped (RPM → cutoff).", [i("in", A, None, "Input value."), i("inLo", A, Some(0.0), "Input range start."), i("inHi", A, Some(1.0), "Input range end."), i("outLo", A, Some(0.0), "Output at inLo."), i("outHi", A, Some(1.0), "Output at inHi.")], SPEC_NONE, OUT, 200;
    Pow = "pow", "x raised to e (curves, perceptual mappings).", [i("in", A, None, "Base, clamped to >= 0."), i("e", A, Some(2.0), "Exponent.")], SPEC_NONE, OUT, 900;
    Gain = "gain", "Scales a signal by a level in dB.", [i("in", A, None, "Input signal."), i("db", A, Some(0.0), "Gain in dB.")], SPEC_NONE, OUT, 600;
    Crossfade = "crossfade", "Equal-power crossfade a→b by t (layer blends).", [i("a", A, None, "Signal at t = 0."), i("b", A, None, "Signal at t = 1."), i("t", A, Some(0.5), "Blend 0..1.")], SPEC_NONE, OUT, 600;
    Saturate = "saturate", "Soft clipping tanh(drive·x) (engines, distortion).", [i("in", A, None, "Input signal."), i("drive", A, Some(1.0), "Pre-gain into the curve.")], SPEC_NONE, OUT, 900;
    Slew = "smooth", "One-pole smoothing of a control signal over `time` seconds.", [i("in", A, None, "Signal to smooth."), i("time", A, Some(0.05), "Time constant in seconds.")], SPEC_NONE, OUT, 300;
    Pan = "pan", "Equal-power stereo pan of a mono signal.", [i("in", A, None, "Mono signal."), i("pos", A, Some(0.0), "-1 left .. 1 right.")], SPEC_NONE, &[o("left", A, "Left channel."), o("right", A, "Right channel.")], 500;
    Switch = "switch", "Selects item `index` (rounded, clamped): audio or clips.", [i("index", A, None, "Item index from 0.")], Some(Variadic { ty: TypeRule::Generic, min: 2, max: 8, doc: "Items of one type." }), &[o("out", TypeRule::Generic, "The selected item.")], 300;
    RandomRange = "random.range", "New uniform random value in lo..hi on each trigger (and at start).", [i("trigger", T, Some(UNCONNECTED), "Draws a new value."), i("seed", A, Some(0.0), "Integer seed (literal)."), i("lo", A, Some(0.0), "Minimum."), i("hi", A, Some(1.0), "Maximum.")], SPEC_NONE, OUT, 200;
    RandomPick = "random.pick", "Picks a random item on each trigger, never repeating the previous one.", [i("trigger", T, Some(UNCONNECTED), "Picks again."), i("seed", A, Some(0.0), "Integer seed (literal).")], Some(Variadic { ty: TypeRule::Generic, min: 2, max: 8, doc: "Items of one type." }), &[o("out", TypeRule::Generic, "The picked item.")], 300;
    Chance = "random.chance", "Passes each trigger with probability p.", [i("trigger", T, None, "Incoming triggers."), i("seed", A, Some(0.0), "Integer seed (literal)."), i("p", A, Some(0.5), "Probability 0..1.")], SPEC_NONE, &[o("out", T, "Passed triggers.")], 200;
    Metro = "metro", "Periodic trigger, first at voice start.", [i("rate", A, Some(1.0), "Triggers per second.")], SPEC_NONE, &[o("out", T, "Ticks.")], 200;
    Start = "start", "One trigger at the voice's first frame.", [], SPEC_NONE, &[o("out", T, "Fires once.")], 50;
    Delay = "trigger.delay", "Delays triggers by `time` seconds (up to 8 pending).", [i("trigger", T, None, "Incoming triggers."), i("time", A, Some(0.1), "Delay in seconds.")], SPEC_NONE, &[o("out", T, "Delayed triggers.")], 200;
    Every = "trigger.every", "Passes every n-th trigger (n rounded, >= 1).", [i("trigger", T, None, "Incoming triggers."), i("n", A, Some(2.0), "Divider.")], SPEC_NONE, &[o("out", T, "Divided triggers.")], 150;
    Seq = "seq", "Step sequencer: advances on each trigger and holds the current step's value.", [i("trigger", T, None, "Advances one step (wrapping).")], Some(Variadic { ty: A, min: 1, max: 8, doc: "Step values." }), OUT, 200;
    Threshold = "threshold", "Fires when the input rises through `level`.", [i("in", A, None, "Watched signal."), i("level", A, Some(0.5), "Crossing level.")], SPEC_NONE, &[o("out", T, "Rising crossings.")], 150;
    Echo = "delay", "Feedback delay line (tails, echoes); `max` seconds are allocated at load.", [i("in", A, None, "Input signal."), i("time", A, Some(0.25), "Delay in seconds, at most `max`."), i("feedback", A, Some(0.0), "Feedback -0.98..0.98."), i("max", A, Some(1.0), "Literal capacity in seconds (<= 10).")], SPEC_NONE, OUT, 700;
}

impl Op {
    /// Resolves a DSL/registry name.
    pub fn from_name(name: &str) -> Option<Op> {
        Op::ALL.iter().copied().find(|op| op.spec().name == name)
    }
    /// Graph registry kind (`audio.lowpass`).
    pub fn kind(self) -> String {
        format!("audio.{}", self.spec().name)
    }
}

impl OpSpec {
    /// Index of a fixed input by name.
    pub fn input(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|i| i.name == name)
    }
    /// Index of an output by name.
    pub fn output(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|o| o.name == name)
    }
    /// Graph port key of positional argument `index` (fixed name or `inN`).
    pub fn port(&self, index: usize) -> String {
        match self.inputs.get(index) {
            Some(input) => input.name.to_owned(),
            None => format!("in{}", index - self.inputs.len()),
        }
    }
}

fn port(ty: Type, direction: Direction, required: bool, doc: &str) -> Port {
    Port {
        ty: ty.name().into(),
        direction,
        required,
        doc: doc.into(),
    }
}

/// Documented `ara-graph` registry entries (`audio.*`), for graph search and validation.
/// Variadic and generic operations validate their instantiated sockets in the adapter.
pub fn registry() -> Result<NodeRegistry, Diagnostic> {
    let mut registry = NodeRegistry::default();
    for op in Op::ALL {
        let spec = op.spec();
        let dynamic =
            spec.variadic.is_some() || spec.outputs.iter().any(|o| o.ty == TypeRule::Generic);
        let mut ports = BTreeMap::new();
        if !dynamic {
            for input in spec.inputs {
                if let TypeRule::Is(ty) = input.ty {
                    ports.insert(
                        input.name.to_owned(),
                        port(ty, Direction::Input, false, input.doc),
                    );
                }
            }
            for output in spec.outputs {
                if let TypeRule::Is(ty) = output.ty {
                    ports.insert(
                        output.name.to_owned(),
                        port(ty, Direction::Output, false, output.doc),
                    );
                }
            }
        }
        registry.add(NodeKind {
            name: op.kind(),
            version: 1,
            doc: spec.doc.into(),
            ports,
            dynamic_ports: dynamic,
        })?;
    }
    for (name, doc) in [
        ("audio.param", "Gameplay-driven value parameter."),
        ("audio.trigger", "Gameplay-fired trigger."),
        ("audio.clip", "Clip parameter, overridable per instance."),
        (
            "audio.output",
            "Graph outputs: out (mono), left/right (stereo), done (ends the voice).",
        ),
    ] {
        registry.add(NodeKind {
            name: name.into(),
            version: 1,
            doc: doc.into(),
            ports: BTreeMap::new(),
            dynamic_ports: true,
        })?;
    }
    Ok(registry)
}
