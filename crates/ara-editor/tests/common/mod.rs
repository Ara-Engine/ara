#![allow(clippy::unwrap_used, dead_code)] // Test fixtures assert each step.
//! Projects the window-launching integration tests open.
use serde_json::{Value, json};
use std::path::PathBuf;

/// A project whose scene uses `models/crate.gltf`; the glTF is `{}`, which fails to import.
pub fn project(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-editor")
        .join(name)
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    let content = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(content.join("levels")).unwrap();
    std::fs::create_dir_all(content.join("models")).unwrap();
    std::fs::create_dir_all(root.join("settings")).unwrap();
    let write = |path: PathBuf, value: Value| {
        std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap()
    };
    write(
        root.join("ara.project.json"),
        json!({"name": "Background capture", "layoutVersion": ara_doc::PROJECT_LAYOUT_VERSION, "main": "levels/main.scene"}),
    );
    write(
        content.join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_back0001","records":{
            "e_box00001":{"name":"Box","Transform":{},"MeshRenderer":{"shape":"box"}},
            "e_crate001":{"name":"Crate","Transform":{"position":[2,0,0]},
                "MeshRenderer":{"model":"@models/crate.gltf#as_crate0001"}}}}),
    );
    std::fs::write(content.join("models/crate.gltf"), "{}").unwrap();
    write(
        content.join("models/crate.gltf.import.json"),
        json!({"$schema":"ara://schema/import/1","id":"as_crate0001","records":{
            "i_settings":{"importer":"ara.gltf","version":"1","settings":{}}}}),
    );
    root
}
