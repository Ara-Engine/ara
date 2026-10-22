//! The op vocabulary: `create`, `set`, `delete` and `move`.
//!
//! An [`Edit`] is a request; applying it to a document checks it and returns the
//! recorded [`Op`], which carries `prev` or `snapshot` so its inverse is free.
//! Transactions (ara-ops) group ops, validate the result and keep history.

use ara_diag::{Diagnostic, codes};
use ara_schema::grammar::is_entity_address;
use ara_schema::{FieldPath, SchemaDb, Step, Validator, path};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::document::{Document, Record, field, is_component_key};
use crate::validate::record_type;
use crate::{AssetId, OrderKey, RecordId};

/// A requested change to one record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Edit {
    /// Add a record.
    Create {
        /// Document asset ID.
        doc: AssetId,
        /// New record ID; its prefix selects the record type.
        id: RecordId,
        /// The record.
        record: Record,
    },
    /// Write or remove (`value` absent) the value at `path`.
    Set {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
        /// Field path inside the record.
        path: FieldPath,
        /// New value; absent removes the key (back to its default, or the component).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        value: Option<Value>,
    },
    /// Remove a record.
    Delete {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
    },
    /// Reparent and/or reorder a record.
    Move {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
        /// New parent address; absent makes it a root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// New sibling order key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<OrderKey>,
    },
}

/// An applied change, with what it replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Op {
    /// A record was added.
    Create {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
        /// The record as created.
        record: Record,
    },
    /// The value at `path` changed.
    Set {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
        /// Field path inside the record.
        path: FieldPath,
        /// New stored value; absent means removed.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        value: Option<Value>,
        /// Previous stored value; absent means it was not stored.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        prev: Option<Value>,
    },
    /// A record was removed.
    Delete {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
        /// The record as it was.
        snapshot: Record,
    },
    /// A record was reparented or reordered.
    Move {
        /// Document asset ID.
        doc: AssetId,
        /// Record ID.
        id: RecordId,
        /// New parent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// New order key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<OrderKey>,
        /// Previous parent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev_parent: Option<String>,
        /// Previous order key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev_order: Option<OrderKey>,
    },
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

impl Op {
    /// The document the op changed.
    pub fn doc(&self) -> &AssetId {
        match self {
            Op::Create { doc, .. }
            | Op::Set { doc, .. }
            | Op::Delete { doc, .. }
            | Op::Move { doc, .. } => doc,
        }
    }

    /// The record the op changed.
    pub fn id(&self) -> &RecordId {
        match self {
            Op::Create { id, .. }
            | Op::Set { id, .. }
            | Op::Delete { id, .. }
            | Op::Move { id, .. } => id,
        }
    }

    /// The op that undoes this one.
    pub fn inverse(&self) -> Op {
        match self.clone() {
            Op::Create { doc, id, record } => Op::Delete {
                doc,
                id,
                snapshot: record,
            },
            Op::Delete { doc, id, snapshot } => Op::Create {
                doc,
                id,
                record: snapshot,
            },
            Op::Set {
                doc,
                id,
                path,
                value,
                prev,
            } => Op::Set {
                doc,
                id,
                path,
                value: prev,
                prev: value,
            },
            Op::Move {
                doc,
                id,
                parent,
                order,
                prev_parent,
                prev_order,
            } => Op::Move {
                doc,
                id,
                parent: prev_parent,
                order: prev_order,
                prev_parent: parent,
                prev_order: order,
            },
        }
    }

    /// The edit that performs this op again.
    pub fn to_edit(&self) -> Edit {
        match self.clone() {
            Op::Create { doc, id, record } => Edit::Create { doc, id, record },
            Op::Delete { doc, id, .. } => Edit::Delete { doc, id },
            Op::Set {
                doc,
                id,
                path,
                value,
                ..
            } => Edit::Set {
                doc,
                id,
                path,
                value,
            },
            Op::Move {
                doc,
                id,
                parent,
                order,
                ..
            } => Edit::Move {
                doc,
                id,
                parent,
                order,
            },
        }
    }

    /// The paths inside the record the op touched; `None` means the whole record.
    pub fn touches(&self) -> Vec<Option<FieldPath>> {
        match self {
            Op::Create { .. } | Op::Delete { .. } => vec![None],
            Op::Set { path, .. } => vec![Some(path.clone())],
            Op::Move { .. } => vec![
                Some(FieldPath::field(field::PARENT)),
                Some(FieldPath::field(field::ORDER)),
            ],
        }
    }
}

impl Edit {
    /// The document the edit targets.
    pub fn doc(&self) -> &AssetId {
        match self {
            Edit::Create { doc, .. }
            | Edit::Set { doc, .. }
            | Edit::Delete { doc, .. }
            | Edit::Move { doc, .. } => doc,
        }
    }
}

fn not_found(id: &RecordId) -> Diagnostic {
    Diagnostic::error(codes::RECORD_NOT_FOUND, format!("no record `{id}`")).record(id.as_str())
}

impl Document {
    /// Applies an edit and returns the recorded op. Checks only what holds regardless
    /// of the other ops in the transaction (record existence, path, value type); the
    /// whole-document checks (parents, loops, records) run when the transaction
    /// commits. A failed edit leaves the document unchanged.
    ///
    /// Only transactions (ara-ops) should call this; everything else edits documents
    /// through commands.
    pub fn apply(&mut self, edit: Edit, db: &SchemaDb) -> Result<Op, Diagnostic> {
        let kind = db.kind(&self.kind).ok_or_else(|| {
            Diagnostic::error(
                codes::UNKNOWN_SCHEMA,
                format!("unknown document kind `{}`", self.kind),
            )
        })?;
        match edit {
            Edit::Create { doc, id, record } => {
                if self.records.contains_key(&id) {
                    return Err(Diagnostic::error(
                        codes::RECORD_EXISTS,
                        format!("record `{id}` already exists"),
                    )
                    .record(id.as_str()));
                }
                record_type(db, kind, &id)?;
                self.records.insert(id.clone(), record.clone());
                Ok(Op::Create { doc, id, record })
            }
            Edit::Delete { doc, id } => {
                let snapshot = self.records.remove(&id).ok_or_else(|| not_found(&id))?;
                Ok(Op::Delete { doc, id, snapshot })
            }
            Edit::Set {
                doc,
                id,
                path,
                value,
            } => {
                let at = |d: Diagnostic| d.record(id.as_str()).path(path.to_string());
                if matches!(path.head(), field::PARENT | field::ORDER) {
                    return Err(at(Diagnostic::error(
                        codes::INVALID_PATH,
                        "`parent` and `order` change through `move`",
                    )));
                }
                let rt = record_type(db, kind, &id)?;
                let rec = self.records.get_mut(&id).ok_or_else(|| not_found(&id))?;
                let resolved = db.resolve_path(rt, &path).map_err(at)?;
                if let Some(v) = &value {
                    let mut diags = Vec::new();
                    Validator::new(db, &mut diags).value(
                        &resolved.ty,
                        resolved.field,
                        v,
                        &path.to_string(),
                    );
                    if let Some(d) = diags.into_iter().find(Diagnostic::is_error) {
                        return Err(d.record(id.as_str()));
                    }
                }
                if let Some(Step::Key { key, .. }) = resolved.steps.first()
                    && resolved.steps.len() > 1
                    && is_component_key(key)
                    && !rec.contains_key(key)
                {
                    return Err(at(Diagnostic::error(
                        codes::COMPONENT_MISSING,
                        format!("`{id}` has no `{key}` component"),
                    )));
                }
                let prev = path::set(rec, &resolved.steps, value.clone())
                    .map_err(|msg| at(Diagnostic::error(codes::INVALID_PATH, msg)))?;
                Ok(Op::Set {
                    doc,
                    id,
                    path,
                    value,
                    prev,
                })
            }
            Edit::Move {
                doc,
                id,
                parent,
                order,
            } => {
                // Parent existence and loops are checked when the transaction commits:
                // ops may pass through such states mid-transaction, and their inverses
                // must always apply.
                if let Some(p) = &parent
                    && !is_entity_address(p)
                {
                    return Err(Diagnostic::error(
                        codes::MISSING_PARENT,
                        format!("`{p}` is not an entity address"),
                    )
                    .record(id.as_str()));
                }
                let rec = self.records.get_mut(&id).ok_or_else(|| not_found(&id))?;
                let prev_parent = replace_str(rec, field::PARENT, parent.clone());
                let prev_order =
                    replace_str(rec, field::ORDER, order.as_ref().map(ToString::to_string))
                        .and_then(|s| OrderKey::parse(&s).ok());
                Ok(Op::Move {
                    doc,
                    id,
                    parent,
                    order,
                    prev_parent,
                    prev_order,
                })
            }
        }
    }

    /// Undoes `op`, which must be the last op applied to this document. Used to roll
    /// back a failed transaction; undo proper applies inverse edits instead.
    pub fn revert(&mut self, op: &Op, db: &SchemaDb) {
        match op {
            Op::Create { id, .. } => {
                self.records.remove(id);
            }
            Op::Delete { id, snapshot, .. } => {
                self.records.insert(id.clone(), snapshot.clone());
            }
            Op::Set {
                id, path: p, prev, ..
            } => {
                let rt = db
                    .kind(&self.kind)
                    .and_then(|k| db.record_type(k, id.prefix()));
                let steps = rt
                    .and_then(|rt| db.resolve_path(rt, p).ok())
                    .map(|r| r.steps);
                if let (Some(rec), Some(steps)) = (self.records.get_mut(id), steps) {
                    // The op succeeded on this state, so writing `prev` back cannot fail.
                    let _ = path::set(rec, &steps, prev.clone());
                }
            }
            Op::Move {
                id,
                prev_parent,
                prev_order,
                ..
            } => {
                if let Some(rec) = self.records.get_mut(id) {
                    replace_str(rec, field::PARENT, prev_parent.clone());
                    replace_str(
                        rec,
                        field::ORDER,
                        prev_order.as_ref().map(ToString::to_string),
                    );
                }
            }
        }
    }
}

fn replace_str(rec: &mut Record, key: &str, value: Option<String>) -> Option<String> {
    let prev = match value {
        Some(v) => rec.insert(key.to_owned(), Value::from(v)),
        None => rec.shift_remove(key),
    };
    prev.and_then(|v| v.as_str().map(str::to_owned))
}
