use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

#[test]
fn initialize_selects_current_importer_and_configuration_is_atomic() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut commands = Registry::builtin(&mut db);
    ara_importers::commands::register(&mut commands, &mut db).expect("import schemas");
    assert!(commands.check_examples(&db).is_empty());
    let mut ws = Workspace::default();
    ws.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_main").expect("scene ID")),
    );
    let mut store = Store::new(db, ws);
    let batch = Batch::new(
        "main.scene",
        [
            (
                "import.initialize",
                json!({"source":"models/crate.GLB","id":"as_crate"}),
            ),
            (
                "import.initialize",
                json!({"source":"audio/music.ogg","id":"as_music","settings":{"usage":"music","decodedBudget":65536}}),
            ),
            (
                "import.configure",
                json!({"source":"models/crate.GLB","settings":{"lodCount":2},"preset":"fast"}),
            ),
        ],
    );
    let output = commands
        .apply(&mut store, &batch, &Session::human())
        .expect("initialize");
    let installed = ara_importers::registry()
        .expect("installed registry")
        .versions();
    for (id, importer) in [("as_crate", "ara.gltf"), ("as_music", "ara.audio")] {
        let doc = store.workspace().doc(id).expect("sidecar");
        let settings = doc.record("i_settings").expect("settings");
        assert_eq!(settings["importer"], importer);
        assert_eq!(settings["version"], installed[importer]);
    }
    assert_eq!(
        store.workspace().index().path_of("as_crate"),
        Some("models/crate.GLB")
    );
    assert_eq!(
        store
            .workspace()
            .doc("as_crate")
            .expect("sidecar")
            .record("i_settings")
            .expect("settings")["settings"]["lodCount"],
        2
    );
    assert!(output.txn.is_some());
    store.undo_last(&Author::Human).expect("undo all sidecars");
    assert!(store.workspace().doc("as_crate").is_none());
    assert!(store.workspace().doc("as_music").is_none());
}
