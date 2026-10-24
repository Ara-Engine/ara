//! Whole-document regeneration retains canonical content, attribution and undo.
#![allow(clippy::unwrap_used)]
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

#[test]
fn replacing_a_document_is_atomic_and_undo_restores_child_records() {
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_components::register(&mut schema);
    let registry = Registry::builtin(&mut schema);
    let mut workspace = Workspace::default();
    workspace.insert(
        "anchor.scene",
        Document::new("scene", AssetId::parse("sc_anchor").unwrap()),
    );
    let mut store = Store::new(schema, workspace);
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_anchor",
                [("doc.create", json!({"path":"test.prefab","id":"pf_test"}))],
            ),
            &Session::human(),
        )
        .unwrap();
    registry.apply(&mut store,&Batch::new("pf_test",[
        ("entity.create",json!({"id":"e_root","components":{"Transform":{}}})),
        ("entity.create",json!({"id":"e_child","parent":"e_root","components":{"Transform":{"position":[1,2,3]}}}))
    ]),&Session::human()).unwrap();
    let before = ara_doc::write(store.workspace().doc("pf_test").unwrap(), store.schema());
    let replacement = Batch::new(
        "sc_anchor",
        [
            ("doc.delete", json!({"document":"@test.prefab#pf_test"})),
            ("doc.create", json!({"path":"test.prefab","id":"pf_test"})),
        ],
    );
    let mut dry = replacement.clone();
    dry.dry_run = true;
    let preview = registry.apply(&mut store, &dry, &Session::human()).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("pf_test").unwrap(), store.schema())
    );
    let applied = registry
        .apply(&mut store, &replacement, &Session::human())
        .unwrap();
    assert_eq!(applied.ops, preview.ops);
    assert!(
        store
            .workspace()
            .doc("pf_test")
            .unwrap()
            .records()
            .is_empty()
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("pf_test").unwrap(), store.schema())
    );
    assert!(
        registry
            .apply(
                &mut store,
                &Batch::new(
                    "pf_test",
                    [("doc.delete", json!({"document":"@test.prefab#pf_test"}))]
                ),
                &Session::human()
            )
            .is_err()
    );
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("pf_test").unwrap(), store.schema())
    );
}
