use super::*;
use std::collections::BTreeMap;

fn blob(byte: char) -> FileState {
    FileState::bytes(&[byte as u8])
}

#[test]
fn authored_destinations_reject_nonportable_components_before_any_file_ops() {
    for path in [
        "props/CON",
        "props/nul.png",
        "props/COM¹.txt",
        "props/bad\"name",
        "props/line\nbreak",
        "props/trailing.",
        "props/trailing ",
    ] {
        assert!(editable(path).is_err(), "{path:?}");
    }
    assert!(editable(&format!("props/{}", "a".repeat(256))).is_err());
    assert!(editable("props/packages/target/builds/node_modules/legitimate.bin").is_ok());
}

#[test]
fn unicode_destination_aliases_follow_shared_filesystem_policy() {
    let mut inventory = inventory(&[("game/Σ.png", blob('a'))]);
    inventory.case_insensitive = true;
    assert!(
        validate_destination(&inventory, &[], "game/source.png", "game/ς.png", Mode::Move).is_err()
    );
    inventory.case_insensitive = false;
    assert!(
        validate_destination(&inventory, &[], "game/source.png", "game/ς.png", Mode::Move).is_ok()
    );
}

fn inventory(files: &[(&str, FileState)]) -> Inventory {
    Inventory {
        token: "inventory-v1".into(),
        authored_prefix: "game".into(),
        case_insensitive: false,
        files: files
            .iter()
            .map(|(path, state)| ((*path).into(), state.clone()))
            .collect(),
        source_paths: BTreeSet::new(),
        consumers: BTreeMap::new(),
        complete: true,
    }
}

#[test]
fn empty_directory_moves_and_deletes_use_reversible_directory_snapshots() {
    let prepared = inventory(&[("game/empty", FileState::directory())]);
    let files: Vec<_> = prepared.files.clone().into_iter().collect();
    let edits = file_plan(
        &files,
        &BTreeSet::new(),
        "game/empty",
        Some("game/renamed"),
        Mode::Move,
        vec![],
    );

    assert_eq!(
        edits,
        [
            ("game/empty".into(), None),
            ("game/renamed".into(), Some(FileState::directory()))
        ]
    );
    assert_eq!(
        file_plan(
            &files,
            &BTreeSet::new(),
            "game/empty",
            None,
            Mode::Delete,
            vec![]
        ),
        [("game/empty".into(), None)]
    );
}

#[test]
fn sidecar_pair_moves_together_but_duplicate_sidecar_is_reserialized() {
    let files = vec![
        ("game/box.png".into(), blob('a')),
        ("game/box.png.import.json".into(), blob('b')),
    ];
    let documents = BTreeSet::from(["game/box.png.import.json"]);
    let moved = file_plan(
        &files,
        &documents,
        "game/box.png",
        Some("game/crate.png"),
        Mode::Move,
        vec![],
    );
    assert!(moved.contains(&("game/box.png.import.json".into(), None)));
    assert!(moved.contains(&("game/crate.png.import.json".into(), Some(blob('b')))));
    let duplicated = file_plan(
        &files,
        &documents,
        "game/box.png",
        Some("game/copy.png"),
        Mode::Duplicate,
        vec![],
    );
    assert_eq!(
        duplicated,
        [("game/copy.png".into(), Some(blob('a')))],
        "new-ID sidecar is written through normal document persistence"
    );
}

#[test]
fn destination_checks_sidecar_collisions_files_as_parents_and_unsaved_documents() {
    let inventory = inventory(&[
        ("game/copy.png.import.json", blob('a')),
        ("game/occupied", blob('b')),
    ]);
    assert!(
        validate_destination(
            &inventory,
            &[],
            "game/box.png",
            "game/copy.png",
            Mode::Duplicate
        )
        .is_err()
    );
    assert!(
        validate_destination(
            &inventory,
            &[],
            "game/box.png",
            "game/occupied/box.png",
            Mode::Move
        )
        .is_err()
    );
    assert!(
        validate_destination(
            &inventory,
            &["game/unsaved.prefab"],
            "game/box.prefab",
            "game/unsaved.prefab",
            Mode::Duplicate
        )
        .is_err()
    );
}

#[test]
fn case_only_move_retires_old_spelling_before_inserting_the_new_pair() {
    let mut inventory = inventory(&[
        ("game/box.png", blob('a')),
        ("game/box.png.import.json", blob('b')),
    ]);
    inventory.case_insensitive = true;
    validate_destination(
        &inventory,
        &["game/box.png.import.json"],
        "game/box.png",
        "game/Box.png",
        Mode::Move,
    )
    .expect("same source identity");
    assert!(
        validate_destination(
            &inventory,
            &[],
            "game/box.png",
            "game/Box.png",
            Mode::Duplicate
        )
        .is_err()
    );
    let files: Vec<_> = inventory.files.clone().into_iter().collect();
    let edits = file_plan(
        &files,
        &BTreeSet::from(["game/box.png.import.json"]),
        "game/box.png",
        Some("game/Box.png"),
        Mode::Move,
        vec![],
    );
    let first_insert = edits
        .iter()
        .position(|(_, state)| state.is_some())
        .expect("new files");
    assert!(
        edits[..first_insert]
            .iter()
            .all(|(_, state)| state.is_none())
    );
    assert_eq!(edits[first_insert].0, "game/Box.png");
}

#[test]
fn incomplete_inventory_cannot_authorize_directory_operations() {
    let mut prepared = inventory(&[("game/empty", FileState::directory())]);
    prepared.complete = false;
    assert!(
        prepared
            .validate()
            .expect_err("background discovery required")
            .message
            .contains("incomplete")
    );
    prepared.complete = true;
    prepared.validate().expect("explicit complete inventory");
}
