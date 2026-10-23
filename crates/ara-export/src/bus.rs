//! Export diagnostics and doctor checks on the diagnostics bus. They keep their
//! `ARA-EXP-*` names (named codes in `ara_diag::catalog`); shell remediation that is
//! not an `ara` command line travels in `data.commands`.

use ara_diag::{Code, Diagnostic, codes};
use serde_json::{Value, json};

use crate::doctor::{Check, Status};
use crate::event;

/// The catalog code for an `ARA-EXP-*` name; unknown names keep their text in the
/// message under `ARA2011`.
fn code(name: &str) -> (Code, Option<&str>) {
    match Code::parse(name) {
        Some(c) => (c, None),
        None => (codes::UNKNOWN_CODE, Some(name)),
    }
}

fn data(context: &Value, commands: &[String]) -> Value {
    json!({ "context": context, "commands": commands })
}

impl From<&event::Diagnostic> for Diagnostic {
    fn from(d: &event::Diagnostic) -> Diagnostic {
        let (code, unknown) = code(&d.code);
        let message = match unknown {
            Some(name) => format!("{name}: {}", d.message),
            None => d.message.clone(),
        };
        Diagnostic::error(code, message).data(data(&d.context, &d.fixes))
    }
}

impl Check {
    /// A failed or degraded check as a bus diagnostic (`None` when it passed).
    pub fn diagnostic(&self) -> Option<Diagnostic> {
        let (code, unknown) = code(&self.code);
        let message = match unknown {
            Some(name) => format!("{name}: {}", self.message),
            None => self.message.clone(),
        };
        let d = match self.status {
            Status::Ok => return None,
            Status::Warning => Diagnostic::warning(code, message),
            Status::Error => Diagnostic::error(code, message),
        };
        Some(d.data(data(&self.context, &self.fixes)))
    }
}

/// Emits failed doctor checks onto the bus (a no-op without a consumer).
pub(crate) fn emit_checks(checks: &[Check]) {
    for d in checks.iter().filter_map(Check::diagnostic) {
        d.emit();
    }
}
