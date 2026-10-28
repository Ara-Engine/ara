//! Read-only conformance checks over an assembled retained surface.
use crate::report::UiDiagnostic;
use ara_ui::{Ui, source::stable_id};
use std::collections::{BTreeMap, BTreeSet};

/// Find colliding identities in the surface's attached tree, including hidden nodes.
///
/// Separately compiled fragments can each pass markup validation while declaring
/// the same ID after assembly. Detached virtual-row storage is deliberately
/// excluded; identities are required to be unique within one mounted surface.
pub fn audit_attached_ids(ui: &Ui) -> Vec<UiDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut identities = BTreeMap::<u64, usize>::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        let Some(node) = ui.nodes.get(index) else {
            continue;
        };
        if !visited.insert(index) {
            continue;
        }
        pending.extend(node.element.children.iter().rev().copied());
        let key = stable_id(&node.element.id);
        if let Some(&original) = identities.get(&key) {
            let first = &ui.nodes[original];
            diagnostics.push(UiDiagnostic {
                code: "ARA4005".into(),
                node: node.element.id.clone(),
                message: format!(
                    "Mounted identities collide: '{}' (node {original}, parent {}) and '{}' (node {index}, parent {})",
                    first.element.id,
                    parent(ui, original),
                    node.element.id,
                    parent(ui, index),
                ),
                suggested_fix: "Give every attached element a unique authored/model-key ID across all fragments on this surface.".into(),
                ..UiDiagnostic::default()
            });
        } else {
            identities.insert(key, index);
        }
    }
    diagnostics
}

fn parent(ui: &Ui, index: usize) -> String {
    ui.nodes[index]
        .element
        .parent
        .and_then(|parent| ui.nodes.get(parent))
        .map_or_else(|| "<root>".into(), |node| format!("'{}'", node.element.id))
}

#[cfg(test)]
mod tests;
