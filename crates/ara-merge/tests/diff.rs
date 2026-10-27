//! `diff` turns one version of a document into another through ordinary ops.

use ara_doc::{AssetIndex, Document, Edit, IdGen, Workspace, read, write};
use ara_merge::diff;
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::SchemaDb;

fn db() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    db
}

fn doc(db: &SchemaDb, records: &str) -> Document {
    let text = format!(
        r#"{{ "$schema": "ara://schema/scene/1", "id": "sc_diff0001", "records": {{ {records} }} }}"#
    );
    read(&text, "d.scene", db, None)
        .doc
        .expect("test document reads")
}

const FROM: &str = r#"
  "e_a": { "name": "A", "order": "a0", "Transform": { "position": [0, 0, 0], "scale": [2, 2, 2] } },
  "e_b": { "name": "B", "parent": "e_a", "order": "a0" },
  "e_c": { "name": "C", "order": "a1" }"#;

const TO: &str = r#"
  "e_a": { "name": "A2", "order": "a0", "Transform": { "position": [1, 0, 0] } },
  "e_b": { "name": "B", "order": "a2", "Transform": { "rotation": [0, 90, 0] } },
  "e_d": { "name": "D", "parent": "e_b", "order": "a0", "Transform": { "scale": [3, 3, 3] } }"#;

#[test]
fn diff_edits_reproduce_the_target_through_a_transaction() {
    let db = db();
    let (from, to) = (doc(&db, FROM), doc(&db, TO));
    let edits = diff(&from, &to);
    let paths: Vec<String> = edits
        .iter()
        .map(|e| match e {
            Edit::Create { id, .. } => format!("create {id}"),
            Edit::Delete { id, .. } => format!("delete {id}"),
            Edit::Move { id, .. } => format!("move {id}"),
            Edit::Set { id, path, .. } => format!("set {id} {path}"),
            Edit::Document { path, .. } => format!("document {path}"),
            Edit::File { path, .. } => format!("file {path}"),
        })
        .collect();
    assert_eq!(
        paths,
        [
            "set e_a Transform.position",
            "set e_a Transform.scale",
            "set e_a name",
            "move e_b",
            "set e_b Transform",
            "delete e_c",
            "create e_d",
        ]
    );
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("d.scene", from);
    let mut store = Store::with_ids(db.clone(), ws, IdGen::seeded(1));
    store
        .transact(TxnMeta::by(Author::Human), edits)
        .expect("the diff commits");
    let got = store.workspace().doc("sc_diff0001").expect("doc");
    assert_eq!(write(got, &db), write(&to, &db));
    assert!(diff(got, &to).is_empty());
}
