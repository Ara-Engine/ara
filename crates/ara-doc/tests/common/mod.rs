//! Loads the fixture project under `tests/fixtures/`.

#![allow(dead_code)]

use std::path::Path;

use ara_doc::{AssetIndex, Document, Workspace, read, read_header};
use ara_schema::SchemaDb;

/// Documents of the fixture project, by project-relative path.
pub const DOCS: [&str; 3] = [
    "prefabs/crate.prefab",
    "prefabs/stack.prefab",
    "levels/arena.scene",
];

pub fn text(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn db() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    let types = ara_json::parse(&text("types/render.types.json")).expect("types file parses");
    let problems = db.add_json_types(&types);
    assert!(problems.is_empty(), "{problems:#?}");
    let problems = db.verify();
    assert!(problems.is_empty(), "{problems:#?}");
    db
}

pub fn index(db: &SchemaDb) -> AssetIndex {
    let mut ix = AssetIndex::new();
    for rel in DOCS {
        let (kind, id) = read_header(&text(rel), db).expect("header");
        ix.insert(id.to_string(), rel, Some(kind));
    }
    ix.insert("ms_crate001", "meshes/crate.glb", None);
    ix.insert("ms_lid00001", "meshes/lid.glb", None);
    ix
}

/// Reads one document, asserting it has no errors.
pub fn load(rel: &str, db: &SchemaDb, ix: &AssetIndex) -> Document {
    let result = read(&text(rel), rel, db, Some(ix));
    let errors: Vec<String> = result
        .diags
        .iter()
        .filter(|d| d.is_error())
        .map(ToString::to_string)
        .collect();
    assert!(errors.is_empty(), "{rel}:\n{}", errors.join("\n"));
    result.doc.expect("document")
}

pub fn workspace(db: &SchemaDb) -> Workspace {
    let ix = index(db);
    let mut ws = Workspace::new(ix.clone());
    for rel in DOCS {
        ws.insert(rel, load(rel, db, &ix));
    }
    ws
}
