//! Keep selection exercises the real schema, Store, property commands and undo.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_cmd::{Batch, Registry, Session};
use ara_ops::Author;
use ara_schema::Schema;

/// Integer fields expose precision loss and exact entity/field selection.
#[derive(Schema)]
#[ara(component)]
#[allow(dead_code)]
struct KeepValue {
    /// First independently selected integer.
    #[ara(default = 0)]
    left: u64,
    /// Second independently selected integer.
    #[ara(default = 0)]
    right: u64,
    /// Nullable authored reference that Keep may explicitly clear.
    link: Option<ara_schema::EntityRef>,
}
fn fixture() -> (ara_ops::Store, Registry) {
    let mut db = SchemaDb::new();
    assert!(ara_runtime::engine::register_schemas(&mut db).is_empty());
    db.register::<KeepValue>();
    let registry = Registry::builtin(&mut db);
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert(
        "main.scene",
        ara_doc::Document::new("scene", ara_doc::AssetId::parse("sc_keeptest").unwrap()),
    );
    let mut store = ara_ops::Store::new(db, workspace);
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_keeptest",
                [
                    (
                        "entity.create",
                        json!({"id":"e_first","components":{"KeepValue":{"left":1,"right":2}}}),
                    ),
                    (
                        "entity.create",
                        json!({"id":"e_second","components":{"KeepValue":{"left":1,"right":2}}}),
                    ),
                ],
            ),
            &Session::human(),
        )
        .unwrap();
    (store, registry)
}
fn change(entity: &str, path: &str, prev: Value, value: Value) -> ara_runtime::live::Change {
    ara_runtime::live::Change {
        entity: entity.into(),
        path: path.into(),
        prev: Some(prev),
        value: Some(value),
        driven: false,
    }
}
fn params(fields: Value) -> PlayKeepParams {
    serde_json::from_value(json!({"session":"test","fields":fields})).unwrap()
}
#[test]
fn exact_pairs_never_form_a_cartesian_product_and_commit_one_undo() {
    let (mut store, registry) = fixture();
    let before = store.workspace().doc("sc_keeptest").unwrap().clone();
    let plan = classify(
        &store,
        "sc_keeptest",
        vec![
            change("e_first", "KeepValue.left", json!(1), json!(11)),
            change("e_first", "KeepValue.right", json!(2), json!(22)),
            change("e_second", "KeepValue.left", json!(1), json!(33)),
            change("e_second", "KeepValue.right", json!(2), json!(44)),
        ],
        None,
    )
    .unwrap();
    let selected_fields = params(
        json!([{"entity":"e_first","path":"KeepValue.left"},{"entity":"e_second","path":"KeepValue.right"}]),
    );
    let selected: Vec<_> = plan
        .candidates
        .iter()
        .filter(|candidate| selected(&selected_fields, candidate))
        .collect();
    assert_eq!(selected.len(), 2);
    let batch = Batch::new(
        "sc_keeptest",
        selected.iter().map(|candidate| {
            (
                "property.set",
                json!({"entity":candidate.entity,"path":candidate.path,"value":candidate.play}),
            )
        }),
    );
    let history = store.history().len();
    let result = registry
        .apply(&mut store, &batch, &Session::human())
        .unwrap();
    assert_eq!(store.history().len(), history + 1);
    let doc = store.workspace().doc("sc_keeptest").unwrap();
    assert_eq!(
        doc.record("e_first").unwrap()["KeepValue"],
        json!({"left":11,"right":2})
    );
    assert_eq!(
        doc.record("e_second").unwrap()["KeepValue"],
        json!({"left":1,"right":44})
    );
    store
        .undo(result.txn.as_ref().unwrap(), Author::Human)
        .unwrap();
    assert_eq!(store.workspace().doc("sc_keeptest"), Some(&before));
}
#[test]
fn explicit_empty_selection_and_driven_fields_require_explicit_opt_in() {
    let (store, _) = fixture();
    let mut runtime = change("e_first", "KeepValue.left", json!(1), json!(11));
    runtime.driven = true;
    let plan = classify(&store, "sc_keeptest", vec![runtime], None).unwrap();
    assert!(!selected(&params(json!([])), &plan.candidates[0]));
    let mut chosen = params(json!([{"entity":"e_first","path":"KeepValue.left"}]));
    assert!(!selected(&chosen, &plan.candidates[0]));
    chosen.include_driven = Some(true);
    assert!(selected(&chosen, &plan.candidates[0]));
    chosen.fields = Some(Vec::new());
    assert!(!selected(&chosen, &plan.candidates[0]));
}
#[test]
fn a_fresh_classification_preserves_concurrent_authored_edits_and_large_integer_conflicts() {
    let (mut store, registry) = fixture();
    let runtime = change("e_first", "KeepValue.left", json!(1), json!(11));
    let first = classify(&store, "sc_keeptest", vec![runtime.clone()], None).unwrap();
    assert!(first.candidates[0].eligible);
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_keeptest",
                [(
                    "property.set",
                    json!({"entity":"e_first","path":"KeepValue.left","value":9}),
                )],
            ),
            &Session::human(),
        )
        .unwrap();
    let fresh = classify(&store, "sc_keeptest", vec![runtime], None).unwrap();
    assert!(!fresh.candidates[0].eligible);
    assert_eq!(fresh.candidates[0].status, "conflict");
    assert!(!super::super::same(
        &Some(json!(u64::MAX)),
        &Some(json!(u64::MAX - 1))
    ));
    let saved = classify(
        &store,
        "sc_keeptest",
        vec![change("e_first", "KeepValue.left", json!(1), json!(9))],
        None,
    )
    .unwrap();
    assert_eq!(saved.candidates[0].status, "unchanged");
}
#[test]
fn missing_entities_removed_components_and_invalid_values_are_not_eligible() {
    let (store, _) = fixture();
    let mut removed = change(
        "e_first",
        "KeepValue",
        json!({"left":1,"right":2}),
        Value::Null,
    );
    removed.value = None;
    let plan = classify(
        &store,
        "sc_keeptest",
        vec![
            change("e_missing", "KeepValue.left", json!(1), json!(11)),
            removed,
            change("e_first", "KeepValue.left", json!(1), json!(-1)),
        ],
        None,
    )
    .unwrap();
    assert!(plan.candidates.iter().all(|candidate| !candidate.eligible));
    assert_eq!(plan.candidates[0].status, "conflict");
    assert_eq!(plan.candidates[1].status, "unsupported");
    assert_eq!(plan.candidates[2].status, "unsupported");
}

#[test]
fn nullable_runtime_field_clear_is_kept_as_null_and_undo_restores_the_reference() {
    let (mut store, registry) = fixture();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_keeptest",
                [(
                    "property.set",
                    json!({"entity":"e_first","path":"KeepValue.link","value":"e_second"}),
                )],
            ),
            &Session::human(),
        )
        .unwrap();
    let before = store.workspace().doc("sc_keeptest").unwrap().clone();
    let changes=super::super::changes(&json!({"changes":[{"entity":"e_first","path":"KeepValue.link","prev":"e_second","value":null,"driven":false}]})).unwrap();
    let plan = classify(&store, "sc_keeptest", changes, None).unwrap();
    assert!(plan.candidates[0].eligible);
    assert_eq!(plan.candidates[0].play, Some(Value::Null));
    let result=registry.apply(&mut store,&Batch::new("sc_keeptest",[("property.set",json!({"entity":"e_first","path":"KeepValue.link","value":plan.candidates[0].play}))]),&Session::human()).unwrap();
    assert_eq!(
        super::super::current_value(
            store
                .workspace()
                .doc("sc_keeptest")
                .unwrap()
                .record("e_first")
                .unwrap(),
            store.schema(),
            "KeepValue.link"
        ),
        None
    );
    store
        .undo(result.txn.as_ref().unwrap(), Author::Human)
        .unwrap();
    assert_eq!(store.workspace().doc("sc_keeptest"), Some(&before));
}

#[test]
fn prefab_instance_fields_are_classified_against_effective_values_and_keep_overrides_only() {
    let (mut store, registry) = fixture();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_keeptest",
                [(
                    "prefab.create",
                    json!({"entity":"e_first","path":"keep.prefab","id":"pf_keeptest"}),
                )],
            ),
            &Session::human(),
        )
        .unwrap();
    let source = store.workspace().doc("pf_keeptest").unwrap().clone();
    let before = store.workspace().doc("sc_keeptest").unwrap().clone();
    let plan = classify(
        &store,
        "sc_keeptest",
        vec![change("e_first", "KeepValue.left", json!(1), json!(11))],
        None,
    )
    .unwrap();
    assert!(plan.candidates[0].eligible);
    let result = registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_keeptest",
                [(
                    "property.set",
                    json!({"entity":"e_first","path":"KeepValue.left","value":11}),
                )],
            ),
            &Session::human(),
        )
        .unwrap();
    assert_eq!(store.workspace().doc("pf_keeptest"), Some(&source));
    assert_eq!(
        store
            .workspace()
            .doc("sc_keeptest")
            .unwrap()
            .record("e_first")
            .unwrap()["overrides"]["KeepValue.left"],
        11
    );
    store
        .undo(result.txn.as_ref().unwrap(), Author::Human)
        .unwrap();
    assert_eq!(store.workspace().doc("sc_keeptest"), Some(&before));
}
