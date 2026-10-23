use crate::Result;
use ara_sim::{Session, Snapshot, StateHash, TickRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// First differing same-tick field, retaining expected/actual values for agent diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Divergence {
    /// First diverging authoritative tick.
    pub tick: u64,
    /// Stable component/field path in the diagnostic state.
    pub field: String,
    /// Recorded authoritative field value.
    pub expected: Value,
    /// Replayed field value.
    pub actual: Value,
}
/// Opt-in divergence replay, including arbitrary initial state and compatibility hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    /// Full restorable initial checkpoint (seed/RNG/schema/scene/queues included).
    pub initial: Snapshot,
    /// Matching native/gameplay module digest.
    pub build: StateHash,
    /// Applied fixed boundaries in authoritative order.
    pub ticks: Vec<TickRecord>,
    /// Same-tick diagnostic state, retained only when this explicit recorder is used.
    pub states: Vec<Value>,
    /// Per-client conditioner seeds; packet replay is separate from gameplay replay.
    pub link_seeds: Vec<u64>,
}
impl Trace {
    /// Encodes a portable binary trace, preserving finite IEEE float bits.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out).map_err(crate::error)?;
        Ok(out)
    }
    /// Decodes a trace and rejects trailing data; replay validates compatibility/state.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut input = std::io::Cursor::new(bytes);
        let trace = ciborium::de::from_reader(&mut input).map_err(crate::error)?;
        if input.position() != bytes.len() as u64 {
            return Err(crate::NetError("trailing trace data".into()));
        }
        Ok(trace)
    }
    /// Begins recording without changing the running simulation.
    pub fn begin(session: &Session, build: StateHash, link_seeds: Vec<u64>) -> Result<Self> {
        Ok(Self {
            initial: session.snapshot(build)?,
            build,
            ticks: Vec::new(),
            states: Vec::new(),
            link_seeds,
        })
    }
    /// Records a completed boundary and its field-level state.
    pub fn push(&mut self, session: &Session, record: TickRecord) -> Result<()> {
        if record.tick != session.tick()
            || record.tick != self.initial.tick() + self.ticks.len() as u64 + 1
        {
            return Err(crate::NetError(
                "trace requires contiguous completed boundaries".into(),
            ));
        }
        self.states.push(session.diagnostic_state()?);
        self.ticks.push(record);
        Ok(())
    }
    /// Restores/replays through ara-sim and reports the first differing field and tick.
    pub fn replay(&self, session: &mut Session) -> Result<Option<Divergence>> {
        if self.ticks.len() != self.states.len() {
            return Err(crate::NetError("trace checkpoint count mismatch".into()));
        }
        session.restore(&self.initial, self.build)?;
        for (record, expected) in self.ticks.iter().zip(&self.states) {
            session.replay_tick(record)?;
            if let Some((field, expected, actual)) =
                difference(expected, &session.diagnostic_state()?, String::new())
            {
                return Ok(Some(Divergence {
                    tick: record.tick,
                    field,
                    expected,
                    actual,
                }));
            }
            if let Some(hash) = record.hash
                && session.state_hash()? != hash
            {
                return Err(crate::NetError(format!(
                    "tick {}: replay hash differs outside diagnostic state",
                    record.tick
                )));
            }
        }
        Ok(None)
    }
}
fn difference(a: &Value, b: &Value, path: String) -> Option<(String, Value, Value)> {
    if a == b {
        return None;
    }
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for k in keys {
                if let Some(d) = difference(
                    a.get(k).unwrap_or(&Value::Null),
                    b.get(k).unwrap_or(&Value::Null),
                    if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    },
                ) {
                    return Some(d);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                if let Some(d) = difference(a, b, format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
        }
        _ => {}
    }
    Some((path, a.clone(), b.clone()))
}
