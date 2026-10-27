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
