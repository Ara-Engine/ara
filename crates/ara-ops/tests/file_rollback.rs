//! File-state transitions must roll back with document ops, including previews.
use ara_doc::{AssetId, AssetIndex, Document, Edit, RecordId, Workspace, file_state::FileState};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::SchemaDb;
use serde_json::json;

fn id(value: &str) -> AssetId {
    AssetId::parse(value).expect("fixture id")
}
fn store() -> Store {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("main.scene", Document::new("scene", id("sc_anchor")));
    ws.observe_file("game/Existing.cs".into(), Some(FileState::bytes(b"before")));
    Store::new(db, ws)
}
fn files(store: &Store) -> Vec<(String, FileState)> {
    store
        .workspace()
        .file_entries()
        .map(|(path, state)| (path.to_owned(), state.clone()))
        .collect()
}
fn file(path: &str, base: Option<FileState>, state: Option<FileState>) -> Edit {
    Edit::File {
        doc: id("sc_anchor"),
        path: path.into(),
        base,
        state,
    }
}

#[test]
fn dry_run_restores_new_directories_and_replaced_file_states() {
    let mut store = store();
    let before = files(&store);
    let mut preview = store.begin(TxnMeta::by(Author::Human));
    preview
        .apply(file("settings", None, Some(FileState::directory())))
        .expect("directory");
    preview
        .apply(file(
            "game/Existing.cs",
            Some(FileState::bytes(b"before")),
            Some(FileState::bytes(b"after")),
        ))
        .expect("replacement");
    assert_eq!(
        preview.store().workspace().file_state("settings"),
        Some(&FileState::directory())
    );
    assert!(preview.check().is_empty());
    assert_eq!(
        preview.rollback().len(),
        2,
        "preview retains its proposed diff"
    );
    assert_eq!(
        files(&store),
        before,
        "preview must restore the entire observed file table"
    );
    assert_eq!(store.revision(), 0);
    assert!(store.history().is_empty());
}

#[test]
fn failed_mixed_file_and_document_batches_restore_both_tables() {
    for fail_at_commit in [false, true] {
        let mut store = store();
        let before = files(&store);
        let mut edits = vec![
            file("settings", None, Some(FileState::directory())),
            Edit::Document {
                doc: id("sc_created"),
                path: "settings/created.scene".into(),
                kind: Some("scene".into()),
            },
            file(
                "settings/new.cs",
                None,
                Some(FileState::bytes(b"new source")),
            ),
        ];
        edits.push(if fail_at_commit {
            Edit::Create {
                doc: id("sc_created"),
                id: RecordId::parse("e_broken").expect("record"),
                record: json!({"name":"Broken","parent":"e_missing"})
                    .as_object()
                    .expect("record")
                    .clone(),
            }
        } else {
            Edit::Set {
                doc: id("sc_created"),
                id: RecordId::parse("e_missing").expect("record"),
                path: "name".parse().expect("path"),
                value: Some(json!("missing entity")),
            }
        });
        assert!(store.transact(TxnMeta::by(Author::Human), edits).is_err());
        assert_eq!(
            files(&store),
            before,
            "failed batch must restore files (commit={fail_at_commit})"
        );
        assert!(store.workspace().doc("sc_created").is_none());
        assert!(store.workspace().find("settings/created.scene").is_none());
        assert_eq!(store.workspace().docs().count(), 1);
        assert_eq!(store.revision(), 0);
        assert!(store.history().is_empty());
    }
}
