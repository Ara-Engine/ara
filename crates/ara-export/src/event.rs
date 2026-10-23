use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Write};

/// Versioned NDJSON event contract. Each value is emitted on exactly one line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Protocol version (currently 1).
    pub version: u32,
    /// Export-local monotonic sequence, starting at 1.
    pub sequence: u64,
    /// Stage producing this event.
    pub stage: String,
    /// Event payload.
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Agent-facing export progress, artifacts and actionable diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    /// Stage started or completed.
    Progress {
        /// `started` or `completed`.
        status: String,
        /// Elapsed milliseconds when complete.
        elapsed_ms: Option<u64>,
    },
    /// Immutable output file with content identity.
    Artifact {
        /// Absolute artifact path.
        path: String,
        /// File length.
        bytes: u64,
        /// BLAKE3 of the entire artifact.
        hash: String,
    },
    /// Nonfatal condition (including explicitly unavailable verification hooks).
    Warning(Diagnostic),
    /// Fatal condition; export returns failure after emitting this event.
    Error(Diagnostic),
    /// Export completed, including report location and elapsed time.
    Finished {
        /// Canonical report path.
        report: String,
        /// Total export milliseconds.
        elapsed_ms: u64,
    },
}

/// Stable coded diagnostic with machine context and runnable remediation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable `ARA-EXP-*` code; human wording is not an API.
    pub code: String,
    /// Human-readable description, excluding secrets.
    pub message: String,
    /// Structured context such as platform, log path and tool exit code.
    pub context: Value,
    /// Commands the caller may run; never executed automatically.
    pub fixes: Vec<String>,
}

/// Output abstraction shared by the protocol, CLI and tests.
pub trait EventSink {
    /// Consumes a complete event; failures abort the export rather than hiding output loss.
    fn emit(&mut self, event: &Event) -> io::Result<()>;
}

/// Writes newline-delimited compact JSON to a caller-owned stream.
pub struct Ndjson<W>(pub W);
impl<W: Write> EventSink for Ndjson<W> {
    fn emit(&mut self, event: &Event) -> io::Result<()> {
        serde_json::to_writer(&mut self.0, event)?;
        self.0.write_all(b"\n")?;
        self.0.flush()
    }
}
impl EventSink for Vec<Event> {
    fn emit(&mut self, event: &Event) -> io::Result<()> {
        self.push(event.clone());
        Ok(())
    }
}
