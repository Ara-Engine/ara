use super::super::{
    import_graph,
    tests::{begin, call, complete, scratch, write_project},
};
use super::*;
use ara_cmd::builtin::{AssetConsumer, AssetInventory, AssetLocalInput};
use ara_host::{Host, HostConfig, Project};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::Arc,
    time::Duration,
};

#[test]
fn untouched_consumers_refresh_exact_raw_native_and_json_input_proofs() {
    let source = b"unchanged source URI declarations";
    let original = b"old referenced bytes";
    let inputs = [
        ("raw.bin", "../assets/raw.bin"),
        ("native.ui", "../assets/native.ui"),
        ("resources.json", "../assets/resources.json"),
    ]
    .into_iter()
    .map(|(path, uri)| AssetLocalInput {
        uri: uri.into(),
        path: format!("assets/{path}"),
        state: FileState::bytes(original),
        blake3: ara_asset::Digest::of(original).to_string(),
    })
    .collect();
    let mut graph = AssetInventory {
        complete: true,
        case_insensitive: cfg!(windows),
        source_paths: BTreeSet::from(["models/model.gltf".into()]),
        consumers: BTreeMap::from([(
            "models/model.gltf".into(),
            AssetConsumer {
                state: FileState::bytes(source),
                blake3: ara_asset::Digest::of(source).to_string(),
                inputs,
            },
        )]),
    };
    let values: [(&str, &[u8]); 3] = [
        ("raw.bin", b"exact new binary bytes"),
        (
            "native.ui",
            b"<!-- ara:asset ui_final -->\n<Panel id=\"root\" />",
        ),
        ("resources.json", b"{\"fonts\":[\"final/font.bin\"]}"),
    ];
    let files = values
        .into_iter()
        .map(|(path, bytes)| {
            (
                format!("assets/{path}"),
                FileState::bytes(bytes),
                ara_asset::Digest::of(bytes).to_string(),
            )
        })
        .collect::<Vec<_>>();
    let mut freshly_validated = graph.clone();
    assert!(
        import_graph::refresh(
            &mut freshly_validated,
            &files,
            &BTreeSet::from([ara_doc::file_state::path_key_with_case(
                "models/model.gltf",
                graph.case_insensitive
            )])
        )
        .is_err(),
        "a cook must not claim remapped input bytes it did not consume"
    );
    import_graph::refresh(&mut graph, &files, &BTreeSet::new()).expect("exact final input proofs");
    let consumer = &graph.consumers["models/model.gltf"];
    assert_eq!(consumer.state, FileState::bytes(source));
    for (input, (path, state, blake3)) in consumer.inputs.iter().zip(&files) {
        assert_eq!(&input.path, path);
        assert_eq!(&input.state, state);
        assert_eq!(&input.blake3, blake3);
    }
    assert!(
        import_graph::refresh(
            &mut graph,
            &[(
                "models/model.gltf".into(),
                FileState::bytes(b"changed source without discovery"),
                ara_asset::Digest::of(b"changed source without discovery").to_string()
            )],
            &BTreeSet::new()
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires built ara-import-worker: set ARA_IMPORT_WORKER and run this test with --ignored"]
fn real_service_replace_shared_outside_model_input_is_one_undoable_transaction() {
    let worker = std::env::var_os("ARA_IMPORT_WORKER")
        .map(std::path::PathBuf::from)
        .expect("set ARA_IMPORT_WORKER to the built production importer executable");
    assert!(
        worker.is_absolute() && worker.is_file(),
        "ARA_IMPORT_WORKER must be an existing absolute executable"
    );
    let scratch = scratch();
    let root = scratch.path();
    write_project(root);
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(authored.join("models")).expect("models");
    fs::create_dir_all(authored.join("shared")).expect("shared inputs");
    let model =
        br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../shared/input.bin","byteLength":4}]}"#;
    fs::write(authored.join("models/outside.gltf"), model).expect("outside model");
    fs::write(authored.join("shared/input.bin"), [1, 2, 3, 4]).expect("original shared input");
    let selected = root.join("selected");
    fs::create_dir(&selected).expect("external selection");
    let incoming = selected.join("input.bin");
    fs::write(&incoming, [5, 6, 7, 8]).expect("incoming replacement");
    let project = Project::open_with(root, |db, registry| {
        ara_importers::commands::register(registry, db)
            .err()
            .into_iter()
            .collect()
    })
    .expect("project with production source schemas");
    let mut host = Host::open(
        project,
        HostConfig {
            debounce: Duration::from_secs(3600),
            watch_every: Duration::from_secs(3600),
            scan_every: Duration::from_secs(3600),
            seed: Some(1942),
            ..HostConfig::default()
        },
    )
    .expect("host");
    host.register(Box::new(super::super::AssetService::new(
        ara_asset::ProcessWorker::new(worker).with_texture_encoder(false),
        Arc::new(|_, _| Ok(())),
    )))
    .expect("real asset service");
    let session = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"why":"Replace a shared raw input without breaking its outside consumer","commands":[{"cmd":"asset.import","args":{"sources":[incoming.to_string_lossy()],"destination":"shared","conflict":"replace"}}]}),
    );
    complete(
        &mut host,
        &session,
        started["job"].as_str().expect("preparation job"),
    );
    assert_eq!(
        fs::read(authored.join("shared/input.bin")).expect("physically replaced input"),
        [5, 6, 7, 8]
    );
    assert_eq!(
        fs::read(authored.join("models/outside.gltf")).expect("unchanged outside consumer"),
        model
    );
    assert_eq!(
        call(&mut host, "history.list", json!({"session":session}))["items"]
            .as_array()
            .expect("history")
            .len(),
        1
    );
    for (method, expected) in [
        ("history.undo", [1, 2, 3, 4]),
        ("history.redo", [5, 6, 7, 8]),
    ] {
        let result = call(&mut host, method, json!({"session":session}));
        if let Some(job) = result["job"].as_str() {
            complete(&mut host, &session, job);
        }
        host.flush();
        assert_eq!(
            fs::read(authored.join("shared/input.bin")).expect("history materialization"),
            expected
        );
    }
}
