use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

#[test]
fn doc_create_material_initializes_a_valid_authoritative_graph_atomically() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_material::register(&mut db).expect("material schema");
    let mut registry = Registry::builtin(&mut db);
    ara_material::commands::register(&mut registry, &mut db);
    let mut workspace = Workspace::default();
    workspace.insert(
        "main.scene",
        Document::new("scene", AssetId::parse("sc_main").expect("ID")),
    );
    let mut store = Store::new(db, workspace);
    let mut batch = Batch::new(
        "main.scene",
        [(
            "doc.create",
            json!({"path":"materials/plain.graph","kind":"material","id":"gr_plain"}),
        )],
    );
    batch.dry_run = true;
    let preview = registry
        .apply(&mut store, &batch, &Session::human())
        .expect("preview");
    batch.dry_run = false;
    let applied = registry
        .apply(&mut store, &batch, &Session::human())
        .expect("create");
    assert_eq!(preview.ops, applied.ops);
    let document = store.workspace().doc("gr_plain").expect("material graph");
    let graph = ara_graph::Graph::from_document(document).expect("graph");
    let material = ara_material::Material::from_graph(&graph).expect("material");
    ara_material::check(&material).expect("valid empty material");
    store
        .undo_last(&Author::Human)
        .expect("undo entire template");
    assert!(store.workspace().doc("gr_plain").is_none());
}
