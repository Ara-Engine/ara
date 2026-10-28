//! Commands consume immutable prepared metadata and emit ordinary reversible ops.
use ara_cmd::{Batch, Command, EditCtx, Registry, Session};
use ara_diag::Diagnostic;
use ara_doc::{
    AssetId, Document, Workspace,
    file_state::{FileState, PreparedInventory},
};
use ara_ops::Store;
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

#[test]
fn source_creation_and_relocation_are_reversible_file_ops_with_checked_absence() {
    let directory = format!("{}/scripts", ara_doc::AUTHORING_ROOT);
    let old_path = format!("{directory}/Before.cs");
    let new_path = format!("{directory}/After.cs");
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = Registry::builtin(&mut db);
    let mut workspace = Workspace::default();
    workspace.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_source").expect("scene")),
    );
    let mut store = Store::new(db, workspace);
    store
        .prepare_files(PreparedInventory {
            entries: [
                (ara_doc::AUTHORING_ROOT.into(), None),
                (directory, None),
                (old_path.clone(), None),
            ]
            .into(),
            ..PreparedInventory::default()
        })
        .expect("prepared absence");
    let session = Session::human();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_source",
                [(
                    "source.replace",
                    json!({"path":old_path,"baseHash":null,"text":"class Before {}\n"}),
                )],
            ),
            &session,
        )
        .expect("typed creation");
    let before = FileState::bytes(b"class Before {}\n");
    assert_eq!(store.workspace().file_state(&old_path), Some(&before));
    store
        .prepare_files(PreparedInventory {
            entries: [
                (old_path.clone(), Some(before.clone())),
                (new_path.clone(), None),
            ]
            .into(),
            ..PreparedInventory::default()
        })
        .expect("rename inventory");
    registry.apply(&mut store, &Batch::new("sc_source", [("source.replace", json!({"path":new_path,"from":old_path,"baseHash":before.hash,"text":"class After {}\n"}))]), &session).expect("typed relocation");
    assert!(store.workspace().file_state(&old_path).is_none());
    store.undo_last(&session.author).expect("undo relocation");
    assert_eq!(store.workspace().file_state(&old_path), Some(&before));
    assert!(store.workspace().file_state(&new_path).is_none());
    store
        .undo_last(&session.author)
        .expect("undo source creation");
    assert_eq!(
        store.workspace().file_entries().count(),
        0,
        "including created empty directories"
    );
}

#[test]
fn source_replacement_accepts_the_same_case_insensitive_cs_suffix_as_the_compiler() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = Registry::builtin(&mut db);
    let mut workspace = Workspace::default();
    workspace.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_source").expect("scene")),
    );
    let mut store = Store::new(db, workspace);
    let before = FileState::bytes(b"class Before {}\n");
    store
        .prepare_files(PreparedInventory {
            entries: [("game/Script.CS".into(), Some(before.clone()))].into(),
            ..PreparedInventory::default()
        })
        .expect("prepared");
    let batch = Batch::new(
        "sc_source",
        [(
            "source.replace",
            json!({"path":"game/Script.CS","baseHash":before.hash,"text":"class After {}\n"}),
        )],
    );
    registry
        .apply(&mut store, &batch, &Session::human())
        .expect("compiler accepts .CS files");
    assert_eq!(
        store.workspace().file_state("game/Script.CS"),
        Some(&FileState::bytes(b"class After {}\n"))
    );
}

/// Delete a prepared entry in this test command.
#[derive(Schema, Deserialize)]
#[ara(params)]
struct Params {
    /// Storage path supplied by the preparing owner.
    path: String,
}

struct PreparedDelete;
impl Command for PreparedDelete {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "test.preparedDelete";
    const SUMMARY: &'static str = "Consume checked file metadata without I/O.";
    fn examples() -> Vec<Value> {
        vec![json!({"path":"media/source.bin"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Params) -> Result<Value, Diagnostic> {
        let metadata = ctx.prepared_metadata().cloned().unwrap_or(Value::Null);
        ctx.set_file(&p.path, None)?;
        Ok(metadata)
    }
}

#[test]
fn commands_receive_opaque_metadata_and_cannot_write_outside_the_checked_inventory() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut registry = Registry::builtin(&mut db);
    registry.command::<PreparedDelete>(&mut db);
    let mut workspace = Workspace::default();
    workspace.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_files").expect("scene")),
    );
    let mut store = Store::new(db, workspace);
    let state = FileState::bytes(b"immutable binary source");
    let metadata = json!({"worker":"fixture", "uris":[{"uri":"mesh.bin","inputHash":state.hash}]});
    store
        .prepare_files(PreparedInventory {
            entries: [("media/source.bin".into(), Some(state.clone()))].into(),
            metadata: metadata.clone(),
            ..PreparedInventory::default()
        })
        .expect("checked facts");
    let batch = |path: &str| Batch::new("sc_files", [(PreparedDelete::NAME, json!({"path":path}))]);
    let session = Session::human();
    let error = registry
        .apply(&mut store, &batch("media/unprepared.bin"), &session)
        .expect_err("unprepared mutation");
    assert!(
        error[0]
            .message
            .contains("outside the checked prepared inventory")
    );
    let result = registry
        .apply(&mut store, &batch("media/source.bin"), &session)
        .expect("typed file delete");
    assert_eq!(result.outputs, [metadata]);
    assert!(store.workspace().file_state("media/source.bin").is_none());
    store
        .undo_last(&session.author)
        .expect("ordinary history inverse");
    assert_eq!(
        store.workspace().file_state("media/source.bin"),
        Some(&state)
    );
}
