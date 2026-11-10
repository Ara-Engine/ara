//! Local Set edits preserve document structure and each record's resolved namespace.
use super::*;
use ara_diag::Diagnostic;
use std::ops::Bound::{Included, Unbounded};

impl EditWorld {
    /// Returns a patch only when the same validated source records still contribute
    /// the same addresses and hierarchy. Every uncertainty uses full resolution.
    pub(super) fn resolve_local(&self, records: &BTreeSet<RecordId>) -> Option<Resolved> {
        if records.is_empty() {
            return None;
        }
        // Registry/schema have no mutable accessor after projection. The workspace
        // changes only through apply_ops, or replacement by a fully validated restore.
        let doc = self.workspace.doc(self.scene.as_str())?;
        let kind = self.registry.schema.kind(&doc.kind)?;
        if records.len() == doc.records().len() {
            return None;
        }
        let base = self.workspace.location(&self.scene);
        let mut problems = Vec::new();
        for id in records {
            let record = doc.record(id.as_str())?;
            ara_doc::validate::check_record(
                &self.registry.schema,
                kind,
                id,
                record,
                &base,
                &mut problems,
            );
        }
        if problems.iter().any(Diagnostic::is_error) {
            // Preserve the full pass's first-error precedence for rejected batches.
            return None;
        }
        let mut patch = Resolved::new();
        for id in records {
            problems.clear();
            let rows = ara_doc::prefab::resolve_record_reporting(
                doc,
                id,
                &self.registry.schema,
                &self.workspace,
                &base,
                Some(&mut problems),
            )?;
            if problems.iter().any(Diagnostic::is_error) {
                return None;
            }
            let root = id.as_str();
            let prefix = format!("{root}/");
            let old = self.resolved.get_key_value(root).into_iter().chain(
                self.resolved
                    .range::<str, _>((Included(prefix.as_str()), Unbounded))
                    .take_while(|(address, _)| address.starts_with(&prefix)),
            );
            if !rows.keys().eq(old.map(|(address, _)| address)) {
                return None;
            }
            for (address, row) in &rows {
                let previous = self.resolved.get(address)?;
                if row.parent != previous.parent
                    || row.order != previous.order
                    || row.source != previous.source
                {
                    return None;
                }
                if row != previous {
                    self.entity(address)?;
                    if let Some(parent) = &row.parent {
                        self.entity(parent)?;
                    }
                }
            }
            patch.extend(rows);
        }
        Some(patch)
    }
}
