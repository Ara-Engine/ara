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

#[test]
fn reflected_project_and_texture_quality_overrides_are_typed_and_reversible() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut commands = Registry::builtin(&mut db);
    ara_importers::commands::register(&mut commands, &mut db).expect("schemas");
    let field = db
        .get("ProjectImportSettings")
        .expect("project type")
        .fields()
        .iter()
        .find(|field| field.name == "preset")
        .expect("reflected preset");
    assert!(!field.doc.is_empty());
    let mut ws = Workspace::default();
    ws.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_main").expect("id")),
    );
    let mut store = Store::new(db, ws);
    commands
        .apply(
            &mut store,
            &Batch::new(
                "main.scene",
                [
                    ("import.project.configure", json!({"preset":"highQuality"})),
                    (
                        "import.initialize",
                        json!({"source":"textures/stone.png","id":"as_stone"}),
                    ),
                ],
            ),
            &Session::human(),
        )
        .expect("configure project");
    assert_eq!(
        store
            .workspace()
            .doc("ip_project")
            .expect("project policy")
            .record("pi_settings")
            .expect("record")["preset"],
        "highQuality"
    );
    let settings = store
        .workspace()
        .doc("as_stone")
        .expect("texture")
        .record("i_settings")
        .expect("record");
    assert!(
        !settings.contains_key("preset"),
        "new sources inherit the project"
    );
    commands
        .apply(
            &mut store,
            &Batch::new(
                "main.scene",
                [(
                    "import.configure",
                    json!({"source":"textures/stone.png","preset":"balanced"}),
                )],
            ),
            &Session::human(),
        )
        .expect("texture override");
    assert_eq!(
        store
            .workspace()
            .doc("as_stone")
            .expect("texture")
            .record("i_settings")
            .expect("record")["preset"],
        "balanced"
    );
    commands
        .apply(
            &mut store,
            &Batch::new(
                "main.scene",
                [(
                    "import.configure",
                    json!({"source":"textures/stone.png","useProjectPreset":true}),
                )],
            ),
            &Session::human(),
        )
        .expect("inherit again");
    assert!(
        !store
            .workspace()
            .doc("as_stone")
            .expect("texture")
            .record("i_settings")
            .expect("record")
            .contains_key("preset")
    );
    store.undo_last(&Author::Human).expect("undo reset");
    assert_eq!(
        store
            .workspace()
            .doc("as_stone")
            .expect("texture")
            .record("i_settings")
            .expect("record")["preset"],
        "balanced"
    );
}
