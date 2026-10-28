//! Read-only enumeration exposes both path namespaces without changing project layout.
use ara_host::project::{AuthoredRole, AuthoredScope, authored_files, authored_root};
use std::{collections::BTreeSet, fs, path::PathBuf};

fn scratch() -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("path-contract-tests");
    fs::create_dir_all(&base).expect("scratch root");
    let root = base.join(format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir(&root).expect("fresh isolated project");
    root
}

#[test]
fn explicit_directory_scan_does_not_append_another_authored_root() {
    let root = scratch();
    let layouts = root.join(".ara/editor");
    fs::create_dir_all(&layouts).expect("editor layouts");
    let graph = layouts.join("animation.graph");
    fs::write(&graph, "{}").expect("private layout");
    let entries =
        authored_files(&layouts, &ara_schema::SchemaDb::new()).expect("directory inventory");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "animation.graph");
    assert_eq!(entries[0].storage_path, "animation.graph");
    assert_eq!(entries[0].scope, AuthoredScope::Authored);
}

#[test]
fn modern_index_separates_scope_and_storage_and_preserves_free_nested_folders() {
    let scratch = scratch();
    let root = scratch.as_path();
    let game = root.join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(game.join("settings")).expect("authored folder");
    fs::create_dir_all(root.join("settings")).expect("project settings");
    fs::create_dir_all(root.join("packages")).expect("reserved packages");
    fs::create_dir_all(root.join("builds")).expect("derived output");
    fs::write(root.join("ara.project.json"), "{\"layoutVersion\":4}").expect("marker");
    fs::write(game.join("settings/project.json"), "{}").expect("authored source");
    fs::write(root.join("settings/project.json"), "{}").expect("project policy");
    fs::write(root.join("packages/hidden.cs"), "").expect("plugin placeholder");
    fs::write(root.join("builds/hidden.scene"), "").expect("build output");
    for folder in ["packages", "target", "builds"] {
        fs::create_dir_all(game.join(folder)).expect("free authored folder");
        fs::write(game.join(folder).join("keep.cs"), "").expect("source");
    }
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    let entries = authored_files(root, &db).expect("readonly index");
    assert_eq!(authored_root(root), game);
    let matching: Vec<_> = entries
        .iter()
        .filter(|entry| entry.path == "settings/project.json")
        .collect();
    assert_eq!(matching.len(), 2);
    assert!(
        matching
            .iter()
            .any(|entry| entry.scope == AuthoredScope::Authored
                && entry.storage_path
                    == format!("{}/settings/project.json", ara_doc::AUTHORING_ROOT))
    );
    assert!(
        matching
            .iter()
            .any(|entry| entry.scope == AuthoredScope::Project
                && entry.storage_path == "settings/project.json"
                && entry.role == AuthoredRole::Preferences)
    );
    for folder in ["packages", "target", "builds"] {
        assert!(
            entries
                .iter()
                .any(|entry| entry.scope == AuthoredScope::Authored
                    && entry.path == format!("{folder}/keep.cs"))
        );
    }
    assert!(
        !entries
            .iter()
            .any(|entry| entry.storage_path == "packages/hidden.cs"
                || entry.storage_path == "builds/hidden.scene")
    );
    let unique: BTreeSet<_> = entries.iter().map(|entry| &entry.storage_path).collect();
    assert_eq!(unique.len(), entries.len());
    assert!(!root.join(".ara").exists(), "read-only index created state");
}

#[test]
fn legacy_projects_are_rejected_before_indexing_or_creating_state() {
    for (marker, authored, label) in [
        ("{}", "levels", "project-root"),
        (r#"{"layoutVersion":3}"#, "game/levels", "game/"),
    ] {
        let root = scratch();
        fs::create_dir_all(root.join(authored)).expect("legacy folder");
        fs::write(root.join("ara.project.json"), marker).expect("marker");
        fs::write(root.join(authored).join("main.scene"), "legacy scene").expect("scene");
        let error = ara_host::Project::open(&root)
            .err()
            .expect("migration required");
        assert_eq!(error.code, ara_diag::codes::PROJECT_MIGRATION_REQUIRED);
        assert!(error.message.contains(label));
        assert!(error.message.contains("ara project migrate"));
        assert_eq!(
            fs::read_to_string(root.join("ara.project.json")).expect("unchanged"),
            marker
        );
        assert_eq!(
            fs::read_to_string(root.join(authored).join("main.scene")).expect("unchanged"),
            "legacy scene"
        );
        assert!(!root.join(".ara").exists());
        assert!(!root.join(ara_doc::AUTHORING_ROOT).exists());
    }
}
