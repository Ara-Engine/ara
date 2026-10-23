//! The export event stream. `--json`: one NDJSON object per line on stdout.
//! Otherwise: short human lines. Every event carries `t_ms` since the command started.
//!
//! Event types: `progress`, `stage` (finished, with `ms`), `artifact`, `check`, `warning`,
//! `error`, `result`. Errors and warnings carry a stable `code` (`ARA-EXP-…`) and `fixes`
//! an agent can run as-is.

use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

/// A machine-actionable remedy attached to a check, warning or error.
#[derive(Clone, Serialize)]
pub struct Fix {
    /// `command` (run it), `env` (set it), or `doc` (read it).
    pub kind: &'static str,
    /// The command line, `NAME=value`, or doc reference.
    pub run: String,
}

impl Fix {
    /// A command to run verbatim.
    pub fn cmd(run: impl Into<String>) -> Self {
        Self {
            kind: "command",
            run: run.into(),
        }
    }

    /// An environment variable to set.
    pub fn env(run: impl Into<String>) -> Self {
        Self {
            kind: "env",
            run: run.into(),
        }
    }

    /// A documentation reference.
    pub fn doc(run: impl Into<String>) -> Self {
        Self {
            kind: "doc",
            run: run.into(),
        }
    }
}

/// Outcome of one doctor check.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Present and usable.
    Ok,
    /// Usable, but something will be degraded (e.g. unsigned builds).
    Warn,
    /// Blocks the lane.
    Error,
}

/// Emits events in JSON or human form.
pub struct Events {
    json: bool,
    started: Instant,
}

impl Events {
    /// `json`: NDJSON mode.
    pub fn new(json: bool) -> Self {
        Self {
            json,
            started: Instant::now(),
        }
    }

    fn emit(&self, mut event: Value, human: String) {
        if self.json {
            event["t_ms"] =
                json!((self.started.elapsed().as_secs_f64() * 1e3 * 10.0).round() / 10.0);
            println!("{event}");
        } else {
            println!("{human}");
        }
    }

    /// A stage started or reported intermediate progress.
    pub fn progress(&self, stage: &str, message: &str) {
        self.emit(
            json!({"type": "progress", "stage": stage, "message": message}),
            format!("  .. {stage}: {message}"),
        );
    }

    /// A stage finished in `ms`.
    pub fn stage(&self, stage: &str, ms: f64, extra: Value) {
        let human = format!(
            "  ok {stage} ({ms:.0} ms){}",
            if extra.is_null() {
                String::new()
            } else {
                format!(" {extra}")
            }
        );
        self.emit(
            json!({"type": "stage", "stage": stage, "ms": ms, "detail": extra}),
            human,
        );
    }

    /// A file the command produced.
    pub fn artifact(&self, kind: &str, path: &str, bytes: u64, extra: Value) {
        self.emit(
            json!({"type": "artifact", "kind": kind, "path": path, "bytes": bytes, "detail": extra}),
            format!("  -> {kind}: {path} ({bytes} bytes) {extra}"),
        );
    }

    /// One doctor check.
    pub fn check(
        &self,
        id: &str,
        status: Status,
        code: Option<&str>,
        message: &str,
        fixes: &[Fix],
        context: Value,
    ) {
        let mark = match status {
            Status::Ok => "ok  ",
            Status::Warn => "WARN",
            Status::Error => "FAIL",
        };
        let mut human = format!("[{mark}] {id}: {message}");
        if let Some(code) = code {
            human.push_str(&format!(" ({code})"));
        }
        for f in fixes {
            human.push_str(&format!("\n         fix ({}): {}", f.kind, f.run));
        }
        self.emit(
            json!({"type": "check", "id": id, "status": status, "code": code, "message": message, "fixes": fixes, "context": context}),
            human,
        );
    }

    /// A non-fatal problem.
    pub fn warning(&self, code: &str, stage: &str, message: &str, fixes: &[Fix], context: Value) {
        let mut human = format!("  WARN {code} [{stage}] {message}");
        for f in fixes {
            human.push_str(&format!("\n       fix ({}): {}", f.kind, f.run));
        }
        self.emit(
            json!({"type": "warning", "code": code, "stage": stage, "message": message, "fixes": fixes, "context": context}),
            human,
        );
    }

    /// A fatal problem; the command exits non-zero after emitting it.
    pub fn error(&self, code: &str, stage: &str, message: &str, fixes: &[Fix], context: Value) {
        let mut human = format!("  ERROR {code} [{stage}] {message}");
        for f in fixes {
            human.push_str(&format!("\n        fix ({}): {}", f.kind, f.run));
        }
        self.emit(
            json!({"type": "error", "code": code, "stage": stage, "message": message, "fixes": fixes, "context": context}),
            human,
        );
    }

    /// The command's final summary.
    pub fn result(&self, value: Value) {
        let human = format!(
            "result: {}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        self.emit(json!({"type": "result", "result": value}), human);
    }
}
