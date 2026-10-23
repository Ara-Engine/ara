//! Name-based shading language. Asset identity comes from the caller/file; stable
//! semantic IDs and order metadata live in the graph, never ordinary material text.

mod lexer;
mod literal;
mod parser;
mod printer;

use crate::{Material, codes};
use ara_diag::{Diagnostic, FixAction};

pub use printer::print;

/// A name-reconciled text edit and nonfatal rename guidance.
#[derive(Debug)]
pub struct Reconciled {
    /// Material using the previous graph's IDs for matching names.
    pub material: Material,
    /// Warnings for possible plain-text renames; each carries a recovery command.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses new authored text with the asset ID supplied by the containing file.
/// For existing materials, use `reconcile` to preserve graph identity and layout.
pub fn parse(source: &str, asset_id: &str) -> Result<Material, Vec<Diagnostic>> {
    parser::parse(source, asset_id, None)
}

/// Reconciles file edits directly against a graph, preserving identities/layout
/// even when the sidecar uses draft payloads or contains unavailable node kinds.
/// Only names/types/roles are taken from the old graph; all program values come
/// from the authored source. The caller retains the graph's editor section.
pub fn reconcile_graph(
    source: &str,
    graph: &ara_graph::Graph,
) -> Result<Reconciled, Vec<Diagnostic>> {
    let previous = Material::name_context(graph).map_err(|d| vec![d])?;
    reconcile(source, &previous)
}

/// Parses an edit by matching unique names, preserving all surviving graph IDs.
/// A plain name change creates a new node; `material.rename` preserves identity.
pub fn reconcile(source: &str, previous: &Material) -> Result<Reconciled, Vec<Diagnostic>> {
    let material = parser::parse(source, &previous.id, Some(previous))?;
    let removed: Vec<_> = previous
        .parameters
        .keys()
        .chain(previous.bindings.keys())
        .filter(|id| material.node_type(id).is_none())
        .collect();
    let added: Vec<_> = material
        .parameters
        .keys()
        .chain(material.bindings.keys())
        .filter(|id| previous.node_type(id).is_none())
        .collect();
    let mut diagnostics = Vec::new();
    for old in removed {
        let candidates: Vec<_> = added
            .iter()
            .filter(|new| {
                material.parameters.contains_key(**new) == previous.parameters.contains_key(old)
            })
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let exact: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|new| material.node_type(new) == previous.node_type(old))
            .collect();
        let candidates = if exact.is_empty() { candidates } else { exact };
        let from = previous.node_name(old).unwrap_or("");
        let mut diagnostic=Diagnostic::warning(codes::PLAIN_RENAME,format!("`{from}` was deleted while new names were created; use material.rename if this was a rename to retain layout and identity")).record(old);
        for new in candidates {
            let to = material.node_name(new).unwrap_or("");
            diagnostic = diagnostic.with_fix(
                format!("Keep `{from}` identity as `{to}`"),
                FixAction::Command {
                    command: "material.rename".into(),
                    params: serde_json::json!({"from":from,"to":to,"restoreId":old}),
                },
            );
        }
        diagnostics.push(diagnostic);
    }
    Ok(Reconciled {
        material,
        diagnostics,
    })
}

/// Creates a machine-applicable source replacement, using the ordinary text command.
pub(crate) fn source_fix(diagnostic: Diagnostic, source: String) -> Diagnostic {
    diagnostic.with_fix(
        "Apply the corrected material text",
        FixAction::Command {
            command: "material.text.apply".into(),
            params: serde_json::json!({"text":source}),
        },
    )
}
