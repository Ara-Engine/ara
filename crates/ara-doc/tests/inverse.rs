//! Property: any sequence of ops can be taken back. Reverting the recorded ops, or
//! applying their inverses as new edits, restores the original document exactly
//! (compared in canonical form).

mod common;

use ara_doc::{Document, Edit, OrderKey, RecordId, key_between, write};
use ara_schema::{FieldPath, SchemaDb};
use proptest::prelude::*;
use proptest::sample::Index;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
enum Choice {
    Create(Index, String),
    Set(Index, &'static str, i8),
    Unset(Index, &'static str),
    Delete(Index),
    Move(Index, Option<Index>),
}

const PATHS: [&str; 6] = [
    "name",
    "Transform",
    "Transform.position",
    "Transform.position.y",
    "Light",
    "Light.intensity",
];

fn choice() -> impl Strategy<Value = Choice> {
    let path = prop::sample::select(PATHS.to_vec());
    prop_oneof![
        (any::<Index>(), "[A-Z][a-z]{0,5}").prop_map(|(i, n)| Choice::Create(i, n)),
        (any::<Index>(), path.clone(), any::<i8>()).prop_map(|(i, p, v)| Choice::Set(i, p, v)),
        (any::<Index>(), path).prop_map(|(i, p)| Choice::Unset(i, p)),
        any::<Index>().prop_map(Choice::Delete),
        (any::<Index>(), prop::option::of(any::<Index>())).prop_map(|(i, p)| Choice::Move(i, p)),
    ]
}

fn value_for(path: &str, v: i8) -> Value {
    match path {
        "name" => json!(format!("n{v}")),
        "Transform" => json!({ "position": [v, 0, 0] }),
        "Transform.position" => json!([0, v, f64::from(v) / 4.0]),
        "Light" => json!({ "kind": "spot", "intensity": v.unsigned_abs() }),
        _ => json!(v.unsigned_abs()),
    }
}

/// Turns an abstract choice into an edit against the current document.
fn edit(doc: &Document, c: &Choice, n: &mut u32) -> Option<Edit> {
    let ids: Vec<&RecordId> = doc.records().keys().collect();
    let pick = |i: &Index| (!ids.is_empty()).then(|| ids[i.index(ids.len())].clone());
    let d = doc.id.clone();
    Some(match c {
        Choice::Create(parent, name) => {
            *n += 1;
            let mut record = serde_json::Map::new();
            record.insert("name".into(), json!(name));
            if let Some(p) = pick(parent) {
                record.insert("parent".into(), json!(p.as_str()));
            }
            Edit::Create {
                doc: d,
                id: RecordId::parse(&format!("e_new{n}")).ok()?,
                record,
            }
        }
        Choice::Set(i, path, v) => Edit::Set {
            doc: d,
            id: pick(i)?,
            path: path.parse().ok()?,
            value: Some(value_for(path, *v)),
        },
        Choice::Unset(i, path) => Edit::Set {
            doc: d,
            id: pick(i)?,
            path: path.parse::<FieldPath>().ok()?,
            value: None,
        },
        Choice::Delete(i) => Edit::Delete {
            doc: d,
            id: pick(i)?,
        },
        Choice::Move(i, parent) => {
            let order: Option<OrderKey> = key_between(None, None).ok();
            Edit::Move {
                doc: d,
                id: pick(i)?,
                parent: parent.as_ref().and_then(pick).map(|p| p.to_string()),
                order,
            }
        }
    })
}

fn scenario(db: &SchemaDb) -> Document {
    common::load("levels/arena.scene", db, &common::index(db))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn apply_then_undo_restores_the_document(choices in prop::collection::vec(choice(), 1..40)) {
        let db = common::db();
        let original = scenario(&db);
        let before = write(&original, &db);

        let mut doc = original.clone();
        let mut ops = Vec::new();
        let mut n = 0;
        for c in &choices {
            let Some(e) = edit(&doc, c, &mut n) else { continue };
            // Rejected edits must leave the document untouched.
            let snapshot = doc.clone();
            match doc.apply(e, &db) {
                Ok(op) => ops.push(op),
                Err(_) => prop_assert_eq!(&doc, &snapshot),
            }
        }

        // Undo as transactions do: inverse edits, newest first.
        let mut undone = doc.clone();
        for op in ops.iter().rev() {
            undone.apply(op.inverse().to_edit(), &db).expect("inverse applies");
        }
        prop_assert_eq!(write(&undone, &db), before.clone());

        // Roll back as failed transactions do.
        for op in ops.iter().rev() {
            doc.revert(op, &db);
        }
        prop_assert_eq!(write(&doc, &db), before);
    }
}
