use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

#[test]
fn bus_authoring_round_trips_through_ops_and_undo() {
    let mut schema = SchemaDb::new();
    ara_audio_mix::register(&mut schema).expect("mixer schema");
    let mut commands = Registry::new();
    ara_audio_mix::commands::register(&mut commands, &mut schema);
    assert!(commands.check_examples(&schema).is_empty());
    let mut workspace = Workspace::default();
    workspace.insert(
        "game.mixer",
        Document::new("mixer", AssetId::parse("mx_game").expect("ID")),
    );
    let mut store = Store::new(schema, workspace);
    let batch = Batch::new(
        "game.mixer",
        [(
            "mixer.bus.set",
            json!({"id":"b_music","bus":{"name":"music","output":"master","gainDb":-6}}),
        )],
    );
    let result = commands
        .apply(&mut store, &batch, &Session::human())
        .expect("typed bus authoring");
    assert!(!result.ops.is_empty());
    let doc = store.workspace().doc("mx_game").expect("document");
    assert_eq!(doc.records().values().next().expect("bus")["name"], "music");
    assert_eq!(
        ara_audio_mix::MixerDesc::from_document(doc)
            .expect("valid mixer")
            .buses
            .len(),
        1
    );
    store.undo_last(&Author::Human).expect("undo");
    assert!(
        store
            .workspace()
            .doc("mx_game")
            .expect("document")
            .records()
            .is_empty()
    );
}
