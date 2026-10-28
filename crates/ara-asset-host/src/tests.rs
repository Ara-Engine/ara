use super::*;

fn scratch() -> tempfile::TempDir {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/asset-host-tests");
    std::fs::create_dir_all(&directory).expect("test root");
    tempfile::tempdir_in(directory).expect("isolated files")
}

fn input(source: &str, bytes: &[u8], dependencies: &[(&str, &[u8])]) -> ImportInput {
    ImportInput {
        preset: Default::default(),
        thread_budget: 1,
        source_path: source.into(),
        source: bytes.into(),
        importer: "ara.gltf".into(),
        version: "1".into(),
        settings: serde_json::json!({}),
        target: ara_asset::TargetPlatform::Linux,
        dependencies: dependencies
            .iter()
            .map(|(uri, bytes)| ((*uri).into(), bytes.to_vec()))
            .collect(),
        asset_dependencies: BTreeMap::new(),
    }
}

#[test]
fn graph_preserves_uris_normalizes_local_inputs_and_blocks_raw_dependency_delete() {
    let root = scratch();
    std::fs::create_dir_all(root.path().join("props")).expect("folder");
    std::fs::create_dir_all(root.path().join("textures")).expect("folder");
    std::fs::write(root.path().join("props/model.gltf"), b"model").expect("source");
    std::fs::write(root.path().join("textures/albedo.png"), b"pixels").expect("input");
    let graph = DependencyInventory::from_snapshots(
        root.path(),
        [input(
            "props/model.gltf",
            b"model",
            &[("../textures/./albedo.png", b"pixels")],
        )],
    )
    .expect("prepared graph");
    let inputs = graph.inputs("props/model.gltf").expect("inventoried");
    assert_eq!(inputs[0].uri, "../textures/./albedo.png");
    assert_eq!(inputs[0].path, "textures/albedo.png");
    assert_eq!(
        graph.users("textures/albedo.png").collect::<Vec<_>>(),
        ["props/model.gltf"]
    );
    assert_eq!(graph.sources().count(), 1);
    assert!(
        graph.inputs("never-imported.gltf").is_none(),
        "unknown must not mean no dependencies"
    );
    graph
        .verify(root.path(), &Cancellation::default())
        .expect("same bytes");
    std::fs::write(root.path().join("textures/albedo.png"), b"edited").expect("edit");
    let error = graph
        .verify(root.path(), &Cancellation::default())
        .expect_err("stale prepared graph");
    assert!(error.to_string().contains("textures/albedo.png"));
}

#[test]
fn missing_escaped_cancelled_and_duplicate_inputs_fail_closed() {
    let root = scratch();
    std::fs::write(root.path().join("model.gltf"), b"model").expect("source");
    let missing = input("model.gltf", b"model", &[("missing.bin", b"mesh")]);
    assert!(DependencyInventory::from_snapshots(root.path(), [missing]).is_err());
    let escaped = input(
        "model.gltf",
        b"model",
        &[("https://example.invalid/x", b"mesh")],
    );
    assert!(DependencyInventory::from_snapshots(root.path(), [escaped]).is_err());
    let source = input("model.gltf", b"model", &[]);
    assert!(
        DependencyInventory::from_snapshots(root.path(), [source.clone(), source.clone()]).is_err()
    );
    let graph = DependencyInventory::from_snapshots(root.path(), [source]).expect("source");
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        graph.verify(root.path(), &cancel),
        Err(Error::Cancelled)
    ));
    std::fs::remove_file(root.path().join("model.gltf")).expect("remove");
    assert!(graph.verify(root.path(), &Cancellation::default()).is_err());
}

#[test]
fn transitive_consumers_are_sorted_and_cycles_terminate() {
    let root = scratch();
    for name in ["a", "b", "c"] {
        std::fs::write(root.path().join(name), name.as_bytes()).expect("input");
    }
    let graph = DependencyInventory::from_snapshots(
        root.path(),
        [
            input("c", b"c", &[("b", b"b")]),
            input("b", b"b", &[("a", b"a")]),
            input("a", b"a", &[("c", b"c")]),
        ],
    )
    .expect("cyclic graph");
    assert_eq!(graph.invalidated_by("a"), ["b".into(), "c".into()].into());
}

#[cfg(unix)]
#[test]
fn verification_rejects_retargeted_source_and_uri_aliases() {
    use std::os::unix::fs::symlink;
    let root = scratch();
    for (name, bytes) in [
        ("model-a", b"model".as_slice()),
        ("model-b", b"model".as_slice()),
        ("a.png", b"pixels".as_slice()),
        ("b.png", b"pixels".as_slice()),
    ] {
        std::fs::write(root.path().join(name), bytes).expect("file");
    }
    symlink("a.png", root.path().join("link.png")).expect("URI alias");
    symlink("model-a", root.path().join("model.gltf")).expect("source alias");
    let graph = DependencyInventory::from_snapshots(
        root.path(),
        [input("model.gltf", b"model", &[("link.png", b"pixels")])],
    )
    .expect("graph");
    graph
        .verify(root.path(), &Cancellation::default())
        .expect("same targets");
    std::fs::remove_file(root.path().join("link.png")).expect("remove alias");
    symlink("b.png", root.path().join("link.png")).expect("retarget URI");
    assert!(
        graph.verify(root.path(), &Cancellation::default()).is_err(),
        "same bytes at a different URI target still invalidate preparation"
    );
    std::fs::remove_file(root.path().join("link.png")).expect("remove alias");
    symlink("a.png", root.path().join("link.png")).expect("restore URI");
    std::fs::remove_file(root.path().join("model.gltf")).expect("remove alias");
    symlink("model-b", root.path().join("model.gltf")).expect("retarget source");
    assert!(
        graph.verify(root.path(), &Cancellation::default()).is_err(),
        "retargeted source alias invalidates preparation too"
    );
}
