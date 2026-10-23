//! Name-based behaviour language. Asset identity comes from the file; stable
//! node IDs live in the graph and are matched by name, never written in text.
//!
//! ```text
//! behavior Guard {
//!   var route: points;
//!   state Patrol {
//!     do patrol(route, speed: 1.5);
//!     on heard() -> Investigate;
//!   }
//!   state Investigate {
//!     do moveTo(noise(), speed: 2.5);
//!     on done -> Patrol;
//!   }
//! }
//! ```
mod lexer;
mod parser;
mod printer;

use crate::{ast::Behavior, codes};
use ara_diag::{Diagnostic, FixAction};

pub use parser::{KEYWORDS, fresh_id};
pub use printer::{expr as print_expr, print};

/// A name-reconciled text edit with nonfatal rename guidance.
#[derive(Debug)]
pub struct Reconciled {
    /// Behaviour using the previous IDs for matching names.
    pub behavior: Behavior,
    /// Warnings for likely plain-text renames, each with an undoable fix.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses new text for asset `asset_id`; new names get deterministic IDs.
pub fn parse(source: &str, asset_id: &str) -> Result<Behavior, Diagnostic> {
    parser::parse(source, asset_id, None)
}

/// Parses an expression (blackboard overrides such as `[vec3(0, 0, 4), vec3(8, 0, 4)]`).
pub fn parse_expr(source: &str) -> Result<crate::ast::Expr, Diagnostic> {
    parser::parse_expr(source)
}

/// Parses an edit of `previous`, keeping the IDs of every surviving name. A
/// state removed while another of the same kind appeared is reported as a
/// possible rename with a `behavior.rename` fix that restores its identity.
pub fn reconcile(source: &str, previous: &Behavior) -> Result<Reconciled, Diagnostic> {
    let behavior = parser::parse(source, &previous.id, Some(previous))?;
    let mut diagnostics = Vec::new();
    let names = |b: &Behavior| -> Vec<(String, &'static str, String)> {
        b.vars
            .iter()
            .map(|v| (v.name.clone(), "var", v.id.clone()))
            .chain(
                b.queries
                    .iter()
                    .map(|q| (q.name.clone(), "query", q.id.clone())),
            )
            .chain(
                b.all_states()
                    .into_iter()
                    .map(|s| (s.name.clone(), "state", s.id.clone())),
            )
            .collect()
    };
    let (old, new) = (names(previous), names(&behavior));
    for (name, kind, old_id) in &old {
        if new.iter().any(|(n, k, _)| n == name && k == kind) {
            continue;
        }
        let added: Vec<_> = new
            .iter()
            .filter(|(n, k, _)| k == kind && !old.iter().any(|(o, ok, _)| o == n && ok == k))
            .collect();
        if added.is_empty() {
            continue;
        }
        let mut d = Diagnostic::warning(
            codes::PLAIN_RENAME,
            format!(
                "`{name}` was deleted while new {kind} names appeared; use behavior.rename to keep its identity and layout"
            ),
        );
        for (to, _, _) in added {
            d = d.with_fix(
                format!("Keep `{name}` identity as `{to}`"),
                FixAction::Command {
                    command: "behavior.rename".into(),
                    params: serde_json::json!({"from": name, "to": to, "restoreId": old_id}),
                },
            );
        }
        diagnostics.push(d);
    }
    Ok(Reconciled {
        behavior,
        diagnostics,
    })
}
