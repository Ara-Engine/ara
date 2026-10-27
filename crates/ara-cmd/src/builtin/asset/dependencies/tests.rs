use super::super::metadata::{AssetConsumer as Consumer, AssetLocalInput as LocalInput};
use super::*;

fn blob(byte: char) -> FileState {
    FileState::bytes(&[byte as u8])
}

fn inventory() -> Inventory {
    let mut inventory = Inventory {
        token: "complete-files-settings-worker-v1".into(),
        authored_prefix: "game".into(),
        case_insensitive: false,
        files: BTreeMap::from([
            ("game/models/model.gltf".into(), blob('a')),
            ("game/textures/albedo.png".into(), blob('b')),
        ]),
        source_paths: BTreeSet::from(["models/model.gltf".into(), "textures/albedo.png".into()]),
        consumers: BTreeMap::from([
            (
                "models/model.gltf".into(),
                Consumer {
                    state: blob('a'),
                    blake3: "a".repeat(64),
                    inputs: vec![LocalInput {
                        uri: "../textures/albedo.png".into(),
                        path: "textures/albedo.png".into(),
                        state: blob('b'),
                        blake3: "b".repeat(64),
                    }],
                },
            ),
            (
                "textures/albedo.png".into(),
                Consumer {
                    state: blob('b'),
                    blake3: "b".repeat(64),
                    inputs: vec![],
                },
            ),
        ]),
        complete: true,
    };
    inventory
        .files
        .insert("game/empty".into(), FileState::directory());
    inventory
}

#[test]
fn unimported_models_block_deleting_or_moving_their_raw_inputs() {
    // No .import.json documents exist: discovery covers current Source roles.
    let inventory = inventory();
    for (mode, target) in [
        (Mode::Delete, None),
        (Mode::Move, Some("game/textures/paint.png")),
    ] {
        let error = prepare(&inventory, "game/textures/albedo.png", target, mode)
            .expect_err("outside consumer");
        assert!(
            error
                .message
                .contains("game/models/model.gltf (local URI ../textures/albedo.png)")
        );
    }
    assert!(
        prepare(
            &inventory,
            "game/textures/albedo.png",
            Some("game/textures/copy.png"),
            Mode::Duplicate
        )
        .is_ok()
    );
}

#[test]
fn model_moves_copy_required_companions_using_storage_reference_mapping() {
    let inventory = inventory();
    let copies = prepare(
        &inventory,
        "game/models/model.gltf",
        Some("game/props/detail/model.gltf"),
        Mode::Move,
    )
    .expect("companion plan");
    assert_eq!(
        copies,
        [("game/props/textures/albedo.png".into(), blob('b'))]
    );
    assert!(
        !copies.iter().any(|(path, _)| path.starts_with("props/")),
        "no file may leak beside game/"
    );
    assert!(
        inventory.files.contains_key("game/textures/albedo.png"),
        "shared original input stays intact"
    );
}

#[test]
fn copied_source_companions_include_their_own_uri_inputs() {
    let mut inventory = inventory();
    inventory
        .files
        .insert("game/nested/child.gltf".into(), blob('c'));
    inventory.source_paths.insert("nested/child.gltf".into());
    inventory.consumers.insert(
        "nested/child.gltf".into(),
        Consumer {
            state: blob('c'),
            blake3: "c".repeat(64),
            inputs: vec![LocalInput {
                uri: "../textures/albedo.png".into(),
                path: "textures/albedo.png".into(),
                state: blob('b'),
                blake3: "b".repeat(64),
            }],
        },
    );
    inventory
        .consumers
        .get_mut("models/model.gltf")
        .expect("parent model")
        .inputs = vec![LocalInput {
        uri: "../nested/child.gltf".into(),
        path: "nested/child.gltf".into(),
        state: blob('c'),
        blake3: "c".repeat(64),
    }];
    assert_eq!(
        prepare(
            &inventory,
            "game/models/model.gltf",
            Some("game/props/detail/model.gltf"),
            Mode::Move
        )
        .expect("transitive companion plan"),
        [
            ("game/props/nested/child.gltf".into(), blob('c')),
            ("game/props/textures/albedo.png".into(), blob('b')),
        ]
    );
    inventory
        .files
        .insert("game/props/textures/albedo.png".into(), blob('d'));
    assert!(
        prepare(
            &inventory,
            "game/models/model.gltf",
            Some("game/props/detail/model.gltf"),
            Mode::Move
        )
        .expect_err("nested companion collision")
        .message
        .contains("overwrite different companion data")
    );
}

#[test]
fn moving_a_consumer_and_its_input_together_needs_no_extra_copy() {
    let mut inventory = inventory();
    inventory.files.remove("game/textures/albedo.png");
    inventory
        .files
        .insert("game/models/albedo.png".into(), blob('b'));
    inventory.source_paths.remove("textures/albedo.png");
    inventory.source_paths.insert("models/albedo.png".into());
    let texture = inventory
        .consumers
        .remove("textures/albedo.png")
        .expect("texture");
    inventory
        .consumers
        .insert("models/albedo.png".into(), texture);
    let model = inventory
        .consumers
        .get_mut("models/model.gltf")
        .expect("model");
    model.inputs[0].uri = "albedo.png".into();
    model.inputs[0].path = "models/albedo.png".into();
    assert!(
        prepare(&inventory, "game/models", Some("game/props"), Mode::Move)
            .expect("move group")
            .is_empty()
    );
}

#[test]
fn companion_collisions_accept_identical_bytes_and_reject_different_data() {
    let mut inventory = inventory();
    inventory
        .files
        .insert("game/props/textures/albedo.png".into(), blob('b'));
    assert!(
        prepare(
            &inventory,
            "game/models/model.gltf",
            Some("game/props/detail/model.gltf"),
            Mode::Duplicate
        )
        .expect("reuse same bytes")
        .is_empty()
    );
    inventory
        .files
        .insert("game/props/textures/albedo.png".into(), blob('c'));
    let error = prepare(
        &inventory,
        "game/models/model.gltf",
        Some("game/props/detail/model.gltf"),
        Mode::Move,
    )
    .expect_err("different bytes");
    assert!(error.message.contains("overwrite different companion data"));
}

#[test]
fn missing_discovery_or_stale_bytes_never_mean_no_consumers() {
    let mut incomplete = inventory();
    incomplete.consumers.remove("models/model.gltf");
    assert!(
        prepare(&incomplete, "game/textures/albedo.png", None, Mode::Delete)
            .expect_err("coverage")
            .message
            .contains("cover")
    );
    let mut stale = inventory();
    stale
        .files
        .insert("game/textures/albedo.png".into(), blob('c'));
    assert!(
        prepare(&stale, "game/textures/albedo.png", None, Mode::Delete)
            .expect_err("digest")
            .message
            .contains("does not match")
    );
}

#[test]
fn windows_case_only_input_rename_keeps_existing_uri_meaning() {
    let mut inventory = inventory();
    inventory.case_insensitive = true;
    assert!(
        prepare(
            &inventory,
            "game/textures/albedo.png",
            Some("game/textures/Albedo.png"),
            Mode::Move
        )
        .expect("same filesystem name")
        .is_empty()
    );
    inventory.case_insensitive = false;
    assert!(
        prepare(
            &inventory,
            "game/textures/albedo.png",
            Some("game/textures/Albedo.png"),
            Mode::Move
        )
        .is_err()
    );
}

#[test]
fn local_uri_resolution_stays_inside_authored_root_and_decodes_only_once() {
    assert_eq!(
        resolve("models/new", "../../textures/base coat.png").expect("inside"),
        "textures/base coat.png"
    );
    assert_eq!(
        resolve("models", "../textures/literal%2Fname.png").expect("literal percent"),
        "textures/literal%2Fname.png"
    );
    for uri in [
        "../../escape.bin",
        "../.ara/blob",
        "https://example.invalid/x",
        "C:/outside.bin",
        "..\\outside.bin",
        "",
    ] {
        assert!(resolve("models", uri).is_err(), "{uri}");
    }
}

#[test]
fn canonical_target_mismatch_and_cross_root_paths_are_rejected() {
    let mut inventory = inventory();
    inventory
        .files
        .insert("game/textures/other.png".into(), blob('b'));
    inventory
        .consumers
        .get_mut("models/model.gltf")
        .expect("model")
        .inputs[0]
        .path = "textures/other.png".into();
    assert!(
        inventory
            .validate()
            .expect_err("URI retarget")
            .message
            .contains("no longer resolves")
    );
    assert!(inventory.reference("settings/albedo.png").is_err());
    assert!(
        inventory.editable("game").is_err(),
        "the authored root itself stays in place"
    );
    assert!(inventory.editable("packages/plugin/file.png").is_err());
}

#[test]
fn legacy_protected_roots_use_the_same_filesystem_alias_policy_as_destinations() {
    let mut inventory = inventory();
    inventory.authored_prefix.clear();
    inventory.case_insensitive = true;
    for path in [
        "Settings/project.json",
        "PACKAGES/plugin/file.bin",
        "Builds/player.exe",
    ] {
        assert!(inventory.editable(path).is_err(), "{path}");
    }
    inventory.authored_prefix = "game".into();
    assert!(
        inventory.editable("game/Settings/authored.bin").is_ok(),
        "project-level names remain available inside a modern authored root"
    );
}
