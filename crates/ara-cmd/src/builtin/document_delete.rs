//! Whole-document removal keeps record snapshots and lifecycle in one transaction.
use crate::{Command, EditCtx, Hints};
use ara_diag::{Diagnostic, codes};
use ara_doc::Edit;
use ara_schema::{AssetRef, Schema};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Parameters of `doc.delete`.
#[derive(Schema, Deserialize)]
#[ara(params, module = "core")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocDeleteParams {
    /// Loaded document path or asset reference to remove. Run from a different
    /// batch document, such as ara.project.json; project settings cannot be deleted.
    pub document: AssetRef,
}

/// Removes a document and its records through reversible ops, retaining history.
pub struct DocDelete;
impl Command for DocDelete {
    type Params = DocDeleteParams;
    type Output = Value;
    const NAME: &'static str = "doc.delete";
    const SUMMARY: &'static str = "Delete a loaded document and its records with history and undo.";
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: true,
    };
    fn examples() -> Vec<Value> {
        vec![json!({"document":"@levels/old.scene#sc_old001"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let read = ctx.read();
        let workspace = read.workspace();
        let id = workspace
            .find(&p.document.0)
            .cloned()
            .ok_or_else(|| Diagnostic::error(codes::DOC_NOT_FOUND, "document is not loaded"))?;
        let document = workspace
            .doc(id.as_str())
            .ok_or_else(|| Diagnostic::error(codes::DOC_NOT_FOUND, "document is not loaded"))?;
        if document.kind == "project" || id == read.doc().id {
            return Err(Diagnostic::error(
                codes::INVALID_PARAMS,
                "doc.delete must target another document; use ara.project.json as the batch document. Project settings cannot be deleted.",
            ));
        }
        let path = workspace
            .path_of(id.as_str())
            .ok_or_else(|| Diagnostic::error(codes::DOC_NOT_FOUND, "document has no loaded path"))?
            .to_owned();
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        for record in document.records().keys() {
            for child in document.subtree(record) {
                if seen.insert(child.clone()) {
                    records.push(child);
                }
            }
        }
        for record in records {
            ctx.apply_edit(Edit::Delete {
                doc: id.clone(),
                id: record,
            })?;
        }
        ctx.apply_edit(Edit::Document {
            doc: id.clone(),
            path: path.clone(),
            kind: None,
        })?;
        Ok(json!({"id":id,"path":path}))
    }
}
