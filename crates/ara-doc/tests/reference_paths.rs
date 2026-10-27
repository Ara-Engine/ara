//! Storage paths stay project-relative; references omit the configured authored root.
use ara_doc::{AUTHORING_ROOT, AssetId, Document, Workspace};

#[test]
fn authored_and_project_paths_keep_distinct_storage_and_reference_identity() {
    let mut workspace = Workspace::default();
    workspace.set_reference_root(AUTHORING_ROOT);
    let logical = "settings/main.scene";
    let storage = workspace.storage_path(logical);
    workspace.insert(
        &storage,
        Document::new("scene", AssetId::parse("sc_main1").expect("ID")),
    );
    workspace.insert_project_document(
        "settings/main.scene",
        Document::new("project", AssetId::parse("pj_policy1").expect("ID")),
    );
    assert_eq!(workspace.reference_path(&storage), logical);
    assert_eq!(workspace.path_of("sc_main1"), Some(storage.as_str()));
    assert_eq!(workspace.index().path_of("sc_main1"), Some(logical));
    assert_eq!(
        workspace
            .find("@settings/main.scene#sc_main1")
            .map(AssetId::as_str),
        Some("sc_main1")
    );
    assert_eq!(
        workspace.find(&storage).map(AssetId::as_str),
        Some("sc_main1")
    );
    assert_eq!(
        workspace.find("settings/main.scene").map(AssetId::as_str),
        Some("pj_policy1")
    );
    assert!(workspace.is_project_document("pj_policy1"));
    assert!(workspace.index().path_of("pj_policy1").is_none());
    assert!(!workspace.is_project_document("sc_main1"));
    let cloned = workspace.clone();
    assert_eq!(cloned.reference_root(), AUTHORING_ROOT);
    assert!(cloned.is_project_document("pj_policy1"));
}

#[test]
fn legacy_workspaces_keep_their_existing_reference_paths() {
    let mut workspace = Workspace::default();
    workspace.insert(
        "levels/main.scene",
        Document::new("scene", AssetId::parse("sc_legacy1").expect("ID")),
    );
    assert_eq!(workspace.reference_root(), "");
    assert_eq!(
        workspace.storage_path("levels/main.scene"),
        "levels/main.scene"
    );
    assert_eq!(
        workspace.reference_path("levels/main.scene"),
        "levels/main.scene"
    );
    assert_eq!(
        workspace.index().path_of("sc_legacy1"),
        Some("levels/main.scene")
    );
}
