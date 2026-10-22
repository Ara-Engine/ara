//! Reading documents: strict JSON, `$schema` dispatch, normalization, validation and
//! reference repair, with every problem reported as a located diagnostic.

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, FixAction, Location, codes};
use ara_schema::SchemaDb;
use serde_json::{Map, Value};

use crate::document::Document;
use crate::refs::{AssetIndex, repair_record};
use crate::validate::{check_record, check_structure, record_type};
use crate::{AssetId, RecordId};

/// Top-level keys, in canonical order.
pub(crate) const HEADER: [&str; 5] = ["$schema", "id", "versions", "$conflicts", "records"];

/// The outcome of [`read`]: the document, if the file was structurally usable, and
/// everything noticed along the way.
#[derive(Debug)]
pub struct ReadResult {
    /// The document, normalized (renamed types and fields, upgraded versions,
    /// repaired references). `None` if the JSON or header was unusable.
    pub doc: Option<Document>,
    /// Problems and notes. The document is only safe to write back if none is an error.
    pub diags: Vec<Diagnostic>,
}

/// Reads a document from `text`. `file` is used in diagnostic locations. With an
/// `index`, `@path#id` references are resolved and repaired; without one they are
/// only checked syntactically.
pub fn read(text: &str, file: &str, db: &SchemaDb, index: Option<&AssetIndex>) -> ReadResult {
    let base = Location {
        file: Some(file.to_owned()),
        ..Location::default()
    };
    let mut diags = Vec::new();
    let doc = read_inner(text, db, index, &base, &mut diags);
    ReadResult { doc, diags }
}

fn read_inner(
    text: &str,
    db: &SchemaDb,
    index: Option<&AssetIndex>,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) -> Option<Document> {
    let root = match ara_json::parse(text) {
        Ok(v) => v,
        Err(e) => {
            let code = if e.duplicate_key {
                codes::DUPLICATE_KEY
            } else {
                codes::JSON_SYNTAX
            };
            out.push(
                Diagnostic::error(code, e.message)
                    .within(base)
                    .line_col(e.line, e.column),
            );
            return None;
        }
    };
    let Value::Object(mut root) = root else {
        out.push(
            Diagnostic::error(codes::TYPE_MISMATCH, "a document must be a JSON object")
                .within(base),
        );
        return None;
    };

    let schema = root
        .get("$schema")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let kind = match db.kind_for_schema(&schema) {
        Ok((kind, _)) => kind.clone(),
        Err(d) => {
            let known: Vec<String> = db
                .kinds()
                .map(|k| format!("`{}`", k.schema_uri()))
                .collect();
            let mut d = d.within(base).path("$schema");
            d.message
                .push_str(&format!(" (known: {})", known.join(", ")));
            out.push(d);
            return None;
        }
    };
    let base = &Location {
        doc: root.get("id").and_then(Value::as_str).map(str::to_owned),
        ..base.clone()
    };

    let id = match root.get("id").and_then(Value::as_str).map(AssetId::parse) {
        Some(Ok(id)) if id.prefix() == kind.id_prefix => id,
        Some(Ok(id)) => {
            out.push(
                Diagnostic::error(
                    codes::INVALID_ID,
                    format!(
                        "{} IDs start with `{}_`, not `{}_`",
                        kind.name,
                        kind.id_prefix,
                        id.prefix()
                    ),
                )
                .path("id")
                .within(base),
            );
            return None;
        }
        Some(Err(msg)) => {
            out.push(
                Diagnostic::error(codes::INVALID_ID, msg)
                    .path("id")
                    .within(base),
            );
            return None;
        }
        None => {
            out.push(
                Diagnostic::error(
                    codes::MISSING_DOC_ID,
                    format!(
                        "missing document `id` (`{}_` followed by 8 base32 characters)",
                        kind.id_prefix
                    ),
                )
                .within(base),
            );
            return None;
        }
    };

    for key in root.keys().filter(|k| !HEADER.contains(&k.as_str())) {
        out.push(
            Diagnostic::error(
                codes::UNKNOWN_FIELD,
                format!("unknown top-level key `{key}`"),
            )
            .path(key.as_str())
            .within(base)
            .with_fix(
                format!("Remove `{key}`"),
                FixAction::Remove {
                    record: None,
                    path: key.clone(),
                },
            ),
        );
    }

    let versions = read_versions(root.get("versions"), base, out);
    let conflicts = match root.shift_remove("$conflicts") {
        Some(Value::Array(c)) => c,
        Some(_) => {
            out.push(
                Diagnostic::error(codes::TYPE_MISMATCH, "`$conflicts` must be an array")
                    .path("$conflicts")
                    .within(base),
            );
            Vec::new()
        }
        None => Vec::new(),
    };
    for c in &conflicts {
        let record = c.get("record").and_then(Value::as_str).unwrap_or_default();
        let path = c.get("path").and_then(Value::as_str).unwrap_or_default();
        out.push(
            Diagnostic::error(
                codes::UNRESOLVED_CONFLICT,
                "unresolved merge conflict; pick a value and delete the `$conflicts` entry",
            )
            .record(record)
            .path(path)
            .within(base),
        );
    }

    let mut doc = Document::new(kind.name.clone(), id);
    doc.conflicts = conflicts;
    let records = match root.shift_remove("records") {
        Some(Value::Object(r)) => r,
        Some(_) => {
            out.push(
                Diagnostic::error(codes::TYPE_MISMATCH, "`records` must be an object")
                    .path("records")
                    .within(base),
            );
            return Some(doc);
        }
        None => {
            out.push(Diagnostic::error(codes::MISSING_REQUIRED, "missing `records`").within(base));
            return Some(doc);
        }
    };

    for (key, value) in records {
        let rid = match RecordId::parse(&key) {
            Ok(rid) => rid,
            Err(msg) => {
                out.push(
                    Diagnostic::error(codes::INVALID_ID, msg)
                        .record(key.as_str())
                        .within(base),
                );
                continue;
            }
        };
        let rbase = Location {
            record: Some(key.clone()),
            ..base.clone()
        };
        let Value::Object(mut rec) = value else {
            out.push(
                Diagnostic::error(codes::TYPE_MISMATCH, "a record must be an object")
                    .within(&rbase),
            );
            continue;
        };
        if let Ok(rt) = record_type(db, &kind, &rid) {
            let mut notes = Vec::new();
            db.normalize_record(rt, &mut rec, &versions, &mut notes);
            out.extend(notes.into_iter().map(|d| d.within(&rbase)));
            if let Some(ix) = index {
                repair_record(db, rt, &mut rec, ix, &rbase, out);
            }
        }
        check_record(db, &kind, &rid, &rec, base, out);
        doc.insert_unchecked(rid, rec);
    }
    check_structure(&doc, &kind, base, out);
    Some(doc)
}

fn read_versions(
    v: Option<&Value>,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) -> BTreeMap<String, u32> {
    let mut versions = BTreeMap::new();
    let Some(v) = v else { return versions };
    let Some(map) = v.as_object() else {
        out.push(
            Diagnostic::error(
                codes::TYPE_MISMATCH,
                "`versions` must map type names to versions",
            )
            .path("versions")
            .within(base),
        );
        return versions;
    };
    for (name, n) in map {
        match n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n >= 1)
        {
            Some(n) => {
                versions.insert(name.clone(), n);
            }
            None => out.push(
                Diagnostic::error(codes::TYPE_MISMATCH, "versions are integers >= 1")
                    .path(format!("versions.{name}"))
                    .within(base),
            ),
        }
    }
    versions
}

/// Parses only the header of a document: its kind and asset ID. Used to build the
/// asset index before full reads.
pub fn read_header(text: &str, db: &SchemaDb) -> Option<(String, AssetId)> {
    let root: Map<String, Value> = ara_json::parse(text).ok()?.as_object()?.clone();
    let (kind, _) = db.kind_for_schema(root.get("$schema")?.as_str()?).ok()?;
    let id = AssetId::parse(root.get("id")?.as_str()?).ok()?;
    Some((kind.name.clone(), id))
}
