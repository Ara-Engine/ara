use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Wire protocol version, independent of engine release.
pub const PROTOCOL_VERSION: u32 = 1;

/// One structured runtime log. Sequence numbers permit incremental polling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    /// Runtime sequence, assigned by the bridge.
    pub sequence: u64,
    /// Simulation tick at which the message was observed.
    pub tick: u64,
    /// `trace`, `debug`, `info`, `warning` or `error`.
    pub level: String,
    /// Subsystem name.
    pub target: String,
    /// Human-readable message.
    pub message: String,
    /// Structured properties, excluding credentials.
    pub fields: BTreeMap<String, Value>,
}

/// Timing distribution in microseconds; no raw per-frame history is retained.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    /// Boundaries for the first ten inclusive buckets; bucket 11 is overflow.
    pub counts: [u64; 11],
    /// Number of accepted finite nonnegative samples.
    pub samples: u64,
    /// Sum in microseconds.
    pub total_us: f64,
    /// Maximum observed microseconds.
    pub max_us: f64,
}

impl Histogram {
    /// Records a finite duration into fixed buckets (0.25 to 128 ms).
    pub fn record(&mut self, us: f64) {
        if !us.is_finite() || us < 0.0 {
            return;
        }
        let bounds = [
            250., 500., 1000., 2000., 4000., 8000., 16000., 32000., 64000., 128000.,
        ];
        let index = bounds.iter().position(|b| us <= *b).unwrap_or(10);
        self.counts[index] += 1;
        self.samples += 1;
        self.total_us += us;
        self.max_us = self.max_us.max(us);
    }
}

/// CPU frame and optional GPU timings supplied by renderer integration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfSummary {
    /// Frame duration histogram.
    pub frame: Histogram,
    /// GPU duration histogram (zero samples until the GPU hook is installed).
    pub gpu: Histogram,
    /// Telemetry batches dropped due to backpressure.
    pub dropped_batches: u64,
}

/// Tick-boundary input intent; simulation integration validates the action name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputAction {
    /// Declared action, such as `move` or `interact`.
    pub action: String,
    /// Axis/button values, each finite and in [-1, 1].
    pub values: Vec<f32>,
    /// Tick at which to apply this intent.
    pub tick: u64,
}

/// One authenticated request. Tokens are never reflected into responses.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Protocol version.
    pub version: u32,
    /// Caller-chosen correlation ID.
    pub id: u64,
    /// Secret supplied in launch configuration, never a URL query parameter.
    pub token: String,
    /// Operation to execute.
    pub command: Command,
}

/// Stable bridge operations shared by desktop and mobile transports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Command {
    /// Returns bridge protocol and installed hooks.
    Hello,
    /// Reads the bounded log ring after this sequence.
    Logs {
        /// Last observed sequence (exclusive).
        after: u64,
    },
    /// Captures a PNG through the renderer hook.
    Screenshot,
    /// Returns timing histograms.
    Perf,
    /// Queues one typed input intent.
    Input(InputAction),
    /// Runs a read-only semantic scene query through the sim hook.
    Scene {
        /// Query expression owned and validated by ara-sim.
        query: Value,
    },
    /// Requests a renderer/profiler capture at a frame boundary.
    Capture {
        /// Integration-defined capture kind.
        kind: String,
    },
}

/// Stable machine-readable integration failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeError {
    /// Stable `ARA-EXP-BRG-*` identifier.
    pub code: String,
    /// What failed.
    pub message: String,
    /// Runnable command or concrete integration action.
    pub fix: String,
}

impl BridgeError {
    /// Creates a typed diagnostic without losing the fix.
    pub fn new(code: &str, message: &str, fix: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            fix: fix.into(),
        }
    }
}

/// Correlated response with exactly one success payload or diagnostic.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    /// Protocol version.
    pub version: u32,
    /// Request correlation ID.
    pub id: u64,
    /// Successful operation data.
    pub result: Option<Value>,
    /// Failure with stable code and fix.
    pub error: Option<BridgeError>,
}

/// Renderer/simulation hooks. Default implementations report missing integration;
/// they never invent screenshots, entities or timing data.
pub trait RuntimeHooks: Send + 'static {
    /// Captures PNG bytes from the renderer at its safe frame boundary.
    fn screenshot(&mut self) -> Result<Vec<u8>, BridgeError> {
        Err(BridgeError::new(
            "ARA-EXP-BRG-010",
            "renderer capture hook is not installed",
            "install RuntimeHooks::screenshot from ara-render",
        ))
    }
    /// Queues input for a simulation tick; never mutates a document.
    fn input(&mut self, _action: InputAction) -> Result<(), BridgeError> {
        Err(BridgeError::new(
            "ARA-EXP-BRG-011",
            "input action hook is not installed",
            "install RuntimeHooks::input from ara-sim",
        ))
    }
    /// Queries semantic scene state without changing it.
    fn scene(&mut self, _query: Value) -> Result<Value, BridgeError> {
        Err(BridgeError::new(
            "ARA-EXP-BRG-012",
            "semantic scene hook is not installed",
            "install RuntimeHooks::scene from ara-sim",
        ))
    }
    /// Requests a capture at the next renderer frame boundary.
    fn capture(&mut self, _kind: &str) -> Result<Value, BridgeError> {
        Err(BridgeError::new(
            "ARA-EXP-BRG-013",
            "capture trigger hook is not installed",
            "install RuntimeHooks::capture from ara-render",
        ))
    }
}

/// Explicit empty integration for headless/tooling runtimes.
pub struct NoHooks;
impl RuntimeHooks for NoHooks {}

/// Owned telemetry batch handed off without borrowing game state.
#[derive(Debug, Default)]
pub struct Telemetry {
    /// Logs to append (at most 256 accepted per batch).
    pub logs: Vec<LogRecord>,
    /// CPU frame durations in microseconds (at most 256 per batch).
    pub frame_us: Vec<f64>,
    /// GPU durations in microseconds (at most 256 per batch).
    pub gpu_us: Vec<f64>,
}
