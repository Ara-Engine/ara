use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

#[test]
fn music_commands_author_valid_music_and_undo_together() {
    let mut db = SchemaDb::new();
    ara_music::register(&mut db).expect("music schema");
    let mut registry = Registry::new();
    ara_music::commands::register(&mut registry, &mut db);
    assert!(registry.check_examples(&db).is_empty());
    let id = AssetId::parse("mu_test").expect("asset ID");
    let mut workspace = Workspace::default();
    workspace.insert("test.music", Document::new("music", id.clone()));
    let mut store = Store::new(db, workspace);
    let mut batch = Batch::new(
        "test.music",
        [
            (
                "music.segment.set",
                json!({"id":"sg_ambient","segment":{"name":"Ambient","bars":8}}),
            ),
            (
                "music.settings.set",
                json!({"settings":{"initial":"Ambient","seed":42}}),
            ),
            (
                "music.segment.set",
                json!({"id":"sg_ambient","segment":{"name":"Ambient","bars":4}}),
            ),
        ],
    );
    batch.dry_run = true;
    let preview = registry
        .apply(&mut store, &batch, &Session::human())
        .expect("preview");
    batch.dry_run = false;
    let applied = registry
        .apply(&mut store, &batch, &Session::human())
        .expect("apply");
    assert_eq!(preview.ops, applied.ops);
    let document = store.workspace().doc(id.as_str()).expect("document");
    let music = ara_music::MusicDesc::from_document(document).expect("valid music");
    assert_eq!(music.settings.seed, 42);
    assert_eq!(document.record("sg_ambient").expect("segment")["bars"], 4);
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
