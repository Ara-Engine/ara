//! Semantic three-way merge of documents (`ara merge`, usable as a git merge driver).
//!
//! Documents are flat maps of records keyed by ID, so merging is per record and,
//! inside a record, per object key, recursively: a change on one side wins over an
//! unchanged other side; equal changes agree; different changes to the same field
//! conflict. Arrays and scalars are atomic. After merging, the structure is checked:
//! a dangling parent or a parent loop produced by combining both sides is a
//! conflict too.
//!
//! On conflict the merged document keeps *ours* (or the side that still has the
//! value, if the other deleted it) and lists every conflict under `$conflicts`, so
//! the file stays strict JSON. `ara check` reports unresolved conflicts until the
//! entries are removed.

mod diff;

use std::collections::BTreeSet;

use ara_diag::{Diagnostic, Location, codes};
use ara_doc::validate::check_structure;
use ara_doc::{Document, Record, field};
use ara_json::json_eq;
use ara_schema::SchemaDb;
use serde_json::{Map, Value};

pub use diff::diff;

/// One field both sides changed differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// Record ID.
    pub record: String,
    /// Field path inside the record; empty for the whole record.
    pub path: String,
    /// The value in the common ancestor (`None`: absent).
    pub base: Option<Value>,
    /// Our value.
    pub ours: Option<Value>,
    /// Their value.
    pub theirs: Option<Value>,
    /// Why it conflicts.
    pub reason: String,
}

impl Conflict {
    /// The `$conflicts` entry.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("record".into(), self.record.clone().into());
        if !self.path.is_empty() {
            m.insert("path".into(), self.path.clone().into());
        }
        for (key, v) in [
            ("base", &self.base),
            ("ours", &self.ours),
            ("theirs", &self.theirs),
        ] {
            if let Some(v) = v {
                m.insert(key.into(), v.clone());
            }
        }
        m.insert("reason".into(), self.reason.clone().into());
        Value::Object(m)
    }

    /// The conflict as a diagnostic located in `file`.
    pub fn to_diagnostic(&self, file: &str) -> Diagnostic {
        Diagnostic::error(codes::MERGE_CONFLICT, self.reason.clone())
            .file(file)
            .record(self.record.clone())
            .path(self.path.clone())
    }
}

/// A merge result.
#[derive(Debug, Clone)]
pub struct Merged {
    /// The merged document, with conflicts recorded in `conflicts`.
    pub doc: Document,
    /// Every conflict.
    pub conflicts: Vec<Conflict>,
}

/// Merges `ours` and `theirs`, both descended from `base`.
pub fn merge(
    base: &Document,
    ours: &Document,
    theirs: &Document,
    db: &SchemaDb,
) -> Result<Merged, Diagnostic> {
    if ours.id != theirs.id || ours.kind != theirs.kind {
        return Err(Diagnostic::error(
            codes::MERGE_ID_MISMATCH,
            format!(
                "cannot merge {} `{}` with {} `{}`",
                ours.kind, ours.id, theirs.kind, theirs.id
            ),
        ));
    }
    let mut doc = Document::new(ours.kind.clone(), ours.id.clone());
    let mut conflicts = Vec::new();
    let ids: BTreeSet<_> = [base, ours, theirs]
        .iter()
        .flat_map(|d| d.records().keys().cloned())
        .collect();
    let empty = Record::new();
    for id in ids {
        let (b, o, t) = (
            base.record(id.as_str()),
            ours.record(id.as_str()),
            theirs.record(id.as_str()),
        );
        let mut m = Merger {
            record: id.to_string(),
            conflicts: &mut conflicts,
        };
        let merged = match (b, o, t) {
            (_, Some(o), Some(t)) if json_eq(&obj(o), &obj(t)) => Some(o.clone()),
            (b, Some(o), Some(t)) => Some(m.object(b.unwrap_or(&empty), o, t, "")),
            (Some(b), None, Some(t)) => m.deleted_one_side(b, t, "ours"),
            (Some(b), Some(o), None) => m.deleted_one_side(b, o, "theirs"),
            (None, Some(r), None) | (None, None, Some(r)) => Some(r.clone()),
            _ => None,
        };
        if let Some(rec) = merged {
            doc.insert_unchecked(id, rec);
        }
    }
    structural_conflicts(&doc, [base, ours, theirs], db, &mut conflicts);
    doc.conflicts = ours.conflicts.clone();
    doc.conflicts
        .extend(conflicts.iter().map(Conflict::to_json));
    Ok(Merged { doc, conflicts })
}

fn obj(r: &Record) -> Value {
    Value::Object(r.clone())
}

struct Merger<'a> {
    record: String,
    conflicts: &'a mut Vec<Conflict>,
}

impl Merger<'_> {
    /// One side deleted the record; keep it only if the other side changed it.
    fn deleted_one_side(&mut self, base: &Record, kept: &Record, deleter: &str) -> Option<Record> {
        if json_eq(&obj(base), &obj(kept)) {
            return None;
        }
        let other = if deleter == "ours" { "theirs" } else { "ours" };
        let (ours, theirs) = if deleter == "ours" {
            (None, Some(obj(kept)))
        } else {
            (Some(obj(kept)), None)
        };
        self.conflicts.push(Conflict {
            record: self.record.clone(),
            path: String::new(),
            base: Some(obj(base)),
            ours,
            theirs,
            reason: format!("{deleter} deleted the record; {other} changed it (kept)"),
        });
        Some(kept.clone())
    }

    /// Merges objects key by key.
    fn object(
        &mut self,
        b: &Map<String, Value>,
        o: &Map<String, Value>,
        t: &Map<String, Value>,
        path: &str,
    ) -> Map<String, Value> {
        let keys: Vec<&String> = o
            .keys()
            .chain(t.keys().filter(|k| !o.contains_key(*k)))
            .chain(
                b.keys()
                    .filter(|k| !o.contains_key(*k) && !t.contains_key(*k)),
            )
            .collect();
        let mut out = Map::new();
        for key in keys {
            let (bv, ov, tv) = (b.get(key), o.get(key), t.get(key));
            let p = join(path, key);
            let merged = if same(ov, tv) || same(tv, bv) {
                ov.cloned()
            } else if same(ov, bv) {
                tv.cloned()
            } else if let (Some(Value::Object(om)), Some(Value::Object(tm))) = (ov, tv) {
                let empty = Map::new();
                let bm = bv.and_then(Value::as_object).unwrap_or(&empty);
                Some(Value::Object(self.object(bm, om, tm, &p)))
            } else {
                self.conflicts.push(Conflict {
                    record: self.record.clone(),
                    path: p,
                    base: bv.cloned(),
                    ours: ov.cloned(),
                    theirs: tv.cloned(),
                    reason: "both sides changed this differently (kept ours)".into(),
                });
                ov.or(tv).cloned()
            };
            if let Some(v) = merged {
                out.insert(key.clone(), v);
            }
        }
        out
    }
}

fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => json_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Appends `key` to a field path: `.key` for identifiers, `[key]` otherwise.
fn join(path: &str, key: &str) -> String {
    let ident = key.starts_with(|c: char| c.is_ascii_alphabetic())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    match (path.is_empty(), ident) {
        (true, true) => key.to_owned(),
        (false, true) => format!("{path}.{key}"),
        _ => format!("{path}[{key}]"),
    }
}

/// Turns parent/loop/root problems of the merged document into conflicts on `parent`.
fn structural_conflicts(
    doc: &Document,
    [base, ours, theirs]: [&Document; 3],
    db: &SchemaDb,
    out: &mut Vec<Conflict>,
) {
    let Some(kind) = db.kind(&doc.kind) else {
        return;
    };
    let mut diags = Vec::new();
    check_structure(doc, kind, &Location::default(), &mut diags);
    let parent = |d: &Document, id: &str| d.record(id).and_then(|r| r.get(field::PARENT)).cloned();
    for d in diags.into_iter().filter(Diagnostic::is_error) {
        let record = d.location.record.clone().unwrap_or_default();
        out.push(Conflict {
            path: if record.is_empty() {
                String::new()
            } else {
                field::PARENT.to_owned()
            },
            base: parent(base, &record),
            ours: parent(ours, &record),
            theirs: parent(theirs, &record),
            reason: format!("combining both sides breaks the hierarchy: {}", d.message),
            record,
        });
    }
}
