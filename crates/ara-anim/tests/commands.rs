use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

#[test]
fn protocol_commands_share_animation_intents_and_atomic_undo() {
    let mut db = SchemaDb::new();
    ara_anim::document::register(&mut db).expect("animation schema");
    let mut registry = Registry::new();
    ara_anim::commands::register(&mut registry, &mut db);
    assert!(registry.check_examples(&db).is_empty());
    let id = AssetId::parse("ac_test").expect("asset ID");
    let mut workspace = Workspace::default();
    workspace.insert("test.animation", Document::new("animation", id.clone()));
    let mut store = Store::new(db, workspace);
    let mut batch = Batch::new(
        "test.animation",
        [
            (
                "animation.create",
                json!({"id":"$rig","asset":{"name":"Rig"}}),
            ),
            (
                "animation.skeleton.set",
                json!({"id":"$rig","value":{"joints":[]}}),
            ),
            (
                "animation.property.set",
                json!({"id":"$rig","path":"name","value":"Hero"}),
            ),
        ],
    );
    batch.dry_run = true;
    let preview = registry
        .apply(&mut store, &batch, &Session::human())
        .expect("dry run");
    assert!(
        store
            .workspace()
            .doc(id.as_str())
            .expect("document")
            .records()
            .is_empty()
    );
    batch.dry_run = false;
    let applied = registry
        .apply(&mut store, &batch, &Session::human())
        .expect("apply");
    assert_eq!(preview.ops, applied.ops);
    let record = store
        .workspace()
        .doc(id.as_str())
        .expect("document")
        .records()
        .values()
        .next()
        .expect("asset");
    assert_eq!(record["name"], "Hero");
    assert!(record.contains_key("skeleton"));
    store.undo_last(&Author::Human).expect("undo");
    assert!(
        store
            .workspace()
            .doc(id.as_str())
            .expect("document")
            .records()
            .is_empty()
    );
}
