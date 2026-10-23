//! Record-level diffs as edits: what turns one version of a document into another.
//! Branch merges (ara-collab) replay a merged document onto the live one with these,
//! so the result lands as an ordinary, reviewable, undoable transaction.

use std::collections::BTreeSet;

use ara_doc::{Document, Edit, Record, field, is_component_key, order_key, parent_of};
use ara_json::json_eq;
use ara_schema::FieldPath;
use serde_json::{Map, Value};

/// The edits that turn `from` into `to` (two versions of one document), in record-ID
/// order: `create` and `delete` for added and removed records, one `move` when
/// `parent` or `order` differ, and `set`s otherwise.
///
/// Sets are field-level inside components (`Light.intensity`); a component added or
/// removed as a whole, built-in fields (`name`, `prefab`, `overrides`, `removed`) and
/// non-identifier keys are set whole. Applying the edits to `from` in order gives
/// `to`'s records. `$conflicts` entries are not diffed.
pub fn diff(from: &Document, to: &Document) -> Vec<Edit> {
    let doc = to.id.clone();
    let ids: BTreeSet<_> = from
        .records()
        .keys()
        .chain(to.records().keys())
        .cloned()
        .collect();
    let mut out = Vec::new();
    for id in ids {
        match (from.record(id.as_str()), to.record(id.as_str())) {
            (None, Some(r)) => out.push(Edit::Create {
                doc: doc.clone(),
                id,
                record: r.clone(),
            }),
            (Some(_), None) => out.push(Edit::Delete {
                doc: doc.clone(),
                id,
            }),
            (Some(a), Some(b)) => {
                if parent_of(a) != parent_of(b) || order_key(a) != order_key(b) {
                    out.push(Edit::Move {
                        doc: doc.clone(),
                        id: id.clone(),
                        parent: parent_of(b).map(str::to_owned),
                        order: order_key(b),
                    });
                }
                let mut sets = Vec::new();
                record_sets(a, b, &mut sets);
                out.extend(sets.into_iter().map(|(path, value)| Edit::Set {
                    doc: doc.clone(),
                    id: id.clone(),
                    path,
                    value,
                }));
            }
            (None, None) => {}
        }
    }
    out
}

/// `(path, new value)` for every key of a record that differs.
fn record_sets(a: &Record, b: &Record, out: &mut Vec<(FieldPath, Option<Value>)>) {
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for key in keys {
        if key == field::PARENT || key == field::ORDER || !is_ident(key) {
            continue;
        }
        let (va, vb) = (a.get(key), b.get(key));
        if same(va, vb) {
            continue;
        }
        let path = FieldPath::field(key.as_str());
        match (va, vb) {
            (Some(Value::Object(oa)), Some(Value::Object(ob))) if is_component_key(key) => {
                component_sets(&path, oa, ob, out);
            }
            _ => out.push((path, vb.cloned())),
        }
    }
}

/// Field-level sets inside a component present on both sides.
fn component_sets(
    path: &FieldPath,
    a: &Map<String, Value>,
    b: &Map<String, Value>,
    out: &mut Vec<(FieldPath, Option<Value>)>,
) {
    if a.keys().chain(b.keys()).any(|k| !is_ident(k)) {
        out.push((path.clone(), Some(Value::Object(b.clone()))));
        return;
    }
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for key in keys {
        let (va, vb) = (a.get(key), b.get(key));
        if !same(va, vb) {
            out.push((path.child(key.as_str()), vb.cloned()));
        }
    }
}

fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => json_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
