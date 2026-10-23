//! Reload reports as diagnostics on the bus (`ARA2020` applied, `ARA2021`
//! escalated to L3, `ARA0926` restart required, `ARA0900` compile failed), so
//! `ara why "did reload fall back to L3"` can explain them.

use ara_diag::bus::{EventId, Meta};
use ara_diag::{Diagnostic, Subject, codes as bus_codes};
use serde_json::json;

use crate::abi::{self, ReloadReport};
use crate::codes;

/// The `ara why` key and the words for an `ESC_*` escalation.
fn escalation(esc: u32) -> (&'static str, &'static str) {
    match esc {
        abi::ESC_FILE_SET => ("file_set", "a file was added, removed or renamed"),
        abi::ESC_STRUCTURAL => ("structural", "a declaration changed"),
        abi::ESC_STATE_MACHINE => ("state_machine", "a changed method is an iterator or async"),
        abi::ESC_EMIT_FAILED => ("emit_failed", "Roslyn could not emit a delta"),
        abi::ESC_COLD => ("cold", "the module was cold (optimized IL)"),
        abi::ESC_DEBUGGER => ("debugger", "a debugger is attached"),
        abi::ESC_CLOSURE => ("closure", "a changed method contains a lambda, local function or query"),
        abi::ESC_CONSTRUCTOR => ("constructor", "a constructor or initializer changed"),
        abi::ESC_GENERATED => ("generated", "generated code changed"),
        abi::ESC_REQUESTED => ("requested", "the host asked for a full image"),
        abi::ESC_RUNTIME_REJECTED => ("runtime_rejected", "the runtime rejected the EnC delta"),
        abi::ESC_CAPABILITY => ("capability", "the runtime lacks the EnC capability the edit needs"),
        abi::ESC_DEPENDENCY => ("dependency", "a referenced module changed its public surface"),
        abi::ESC_RELINK => ("relink", "a referenced module got a new generation"),
        _ => ("unknown", "an unknown escalation"),
    }
}

impl ReloadReport {
    /// The report as a bus diagnostic about `module`; `None` for first loads.
    pub fn diagnostic(&self, module: &str) -> Option<(Diagnostic, Meta)> {
        let level = match self.kind {
            abi::RELOAD_L1 => "L1",
            abi::RELOAD_L2 => "L2",
            abi::RELOAD_L3 => "L3",
            abi::RELOAD_L4 => "L4",
            abi::RELOAD_ERROR => "error",
            _ => return None,
        };
        let total_ms = self.server_us as f64 / 1e3
            + self.t_applied.saturating_sub(self.t_received) as f64 / 1e6;
        let (reason, words) = escalation(self.escalation);
        let data = json!({
            "module": module,
            "level": level,
            "reason": (self.escalation != abi::ESC_NONE).then_some(reason),
            "generation": self.generation,
            "cascade": self.cascade,
            "origin": if self.origin == abi::ORIGIN_AGENT { "agent" } else { "human" },
            "totalMs": (total_ms * 10.0).round() / 10.0,
            "emitUs": self.emit_us,
            "loadUs": self.load_us,
            "applyUs": self.apply_us,
        });
        let key = format!("reload:{module}:{level}:{reason}");
        let d = match self.kind {
            abi::RELOAD_ERROR => Diagnostic::error(
                codes::COMPILE_ERROR,
                format!("`{module}` did not reload: {} compile errors; the last good code keeps running", self.error_count),
            ),
            abi::RELOAD_L4 => Diagnostic::warning(
                codes::RESTART_REQUIRED,
                format!("`{module}` cannot reload in this process ({words}); restart play from a snapshot"),
            ),
            abi::RELOAD_L3 if self.escalation != abi::ESC_NONE => Diagnostic::info(
                bus_codes::RELOAD_ESCALATED,
                format!("`{module}` reloaded at L3 (a full module swap, {total_ms:.0} ms) instead of in place: {words} ({reason})"),
            ),
            _ => Diagnostic::info(
                bus_codes::RELOAD_APPLIED,
                format!("`{module}` reloaded at {level} in {total_ms:.0} ms"),
            ),
        };
        let meta = Meta::default().dedup(&key);
        Some((d.subject(Subject::module(module)).data(data), meta))
    }

    /// Emits the report onto the bus (a no-op without a consumer).
    pub fn emit(&self, module: &str) -> Option<EventId> {
        let (d, meta) = self.diagnostic(module)?;
        d.emit_with(meta)
    }
}
