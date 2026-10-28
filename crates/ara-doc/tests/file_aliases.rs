//! The prepared view and reversible file ops must agree on filesystem aliases.
#![cfg(any(windows, target_os = "macos"))]
use ara_doc::{
    AssetId, AssetIndex, Edit, Workspace,
    file_state::{FileState, PreparedInventory, path_key, same_path},
};

#[test]
#[cfg(any(windows, target_os = "macos"))]
fn non_ascii_aliases_share_prepared_state_and_reject_a_stale_base() {
    let before = FileState::bytes(b"before");
    let mut inventory = PreparedInventory::default();
    inventory
        .entries
        .insert("game/Σ.cs".into(), Some(before.clone()));
    assert!(inventory.contains("game/ς.cs"));
    assert!(same_path("game/Σ.cs", "game/σ.cs"));
    assert_eq!(path_key("game/ς.cs"), path_key("game/Σ.cs"));
    let mut workspace = Workspace::new(AssetIndex::new());
    workspace.prepare_files(inventory).expect("prepared");
    assert_eq!(workspace.file_state("game/σ.cs"), Some(&before));
    let error = workspace
        .apply(
            Edit::File {
                doc: AssetId::parse("sc_alias").expect("id"),
                path: "game/ς.cs".into(),
                base: Some(FileState::bytes(b"stale")),
                state: None,
            },
            &ara_schema::SchemaDb::new(),
        )
        .expect_err("stale aliased source");
    assert_eq!(error.code, ara_diag::codes::STALE_BASE);
    assert_eq!(workspace.file_state("game/Σ.cs"), Some(&before));
}

#[test]
#[cfg(windows)]
fn windows_conservative_keys_keep_expansions_and_normalization_distinct() {
    assert!(same_path("game/ı.cs", "GAME/I.CS"));
    assert!(!same_path("game/İ.cs", "game/i.cs"));
    assert!(!same_path("game/ß.cs", "game/SS.cs"));
    assert!(!same_path("game/é.cs", "game/e\u{301}.cs"));
    assert_eq!(
        path_key("game/Test.cs"),
        "game/test.cs",
        "existing ASCII conflict ids stay stable"
    );
}

#[test]
fn contradictory_alias_observations_are_refused_before_entering_the_workspace() {
    let mut inventory = PreparedInventory::default();
    inventory
        .entries
        .insert("game/Σ.cs".into(), Some(FileState::bytes(b"old")));
    inventory.entries.insert("game/ς.cs".into(), None);
    assert_eq!(
        inventory
            .validate()
            .expect_err("same path cannot be both present and absent")
            .code,
        ara_diag::codes::INVALID_PARAMS
    );
    inventory.entries.remove("game/ς.cs");
    inventory
        .directories
        .insert("game".into(), vec!["game/Σ.cs".into(), "game/ς.cs".into()]);
    assert!(
        inventory.validate().is_err(),
        "a complete listing must not contain duplicate aliases"
    );
}
