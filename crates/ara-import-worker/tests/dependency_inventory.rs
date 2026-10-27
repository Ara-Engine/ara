//! Dependency metadata uses the actual disposable native importer, without cooking.
use ara_asset::{Cancellation, CookPreset, ProcessWorker, SourceAsset, TargetPlatform};
use ara_asset_host::DependencyInventory;
use std::{collections::BTreeMap, path::Path};

#[test]
fn unimported_model_uri_and_settings_inputs_are_safe_delete_blockers() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/dependency-inventory");
    std::fs::create_dir_all(&target).expect("test root");
    let root = tempfile::tempdir_in(target).expect("isolated authored root");
    std::fs::create_dir_all(root.path().join("feature")).expect("feature folder");
    std::fs::write(root.path().join("feature/model.gltf"),
        br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"mesh.bin","byteLength":4}],"images":[{"uri":"../color.png"}]}"#).expect("model");
    std::fs::write(root.path().join("feature/mesh.bin"), b"mesh").expect("mesh");
    std::fs::write(root.path().join("color.png"), b"color").expect("image");
    std::fs::write(root.path().join("feature/opacity.png"), b"opacity").expect("settings input");
    let worker =
        ProcessWorker::new(env!("CARGO_BIN_EXE_ara-import-worker")).with_texture_encoder(false);
    let versions = worker.versions().expect("installed importers");
    let source = SourceAsset {
        id: "as_model".into(),
        path: "feature/model.gltf".into(),
        importer: "ara.gltf".into(),
        version: versions["ara.gltf"].clone(),
        preset: Some(CookPreset::Fast),
        thread_budget: 1,
        target: TargetPlatform::Linux,
        settings: serde_json::json!({"opacityMaps":{"#0":"opacity.png"}}),
        asset_dependencies: BTreeMap::new(),
    };
    let graph = DependencyInventory::prepare(
        root.path(),
        root.path(),
        &[source],
        &worker,
        &Cancellation::default(),
    )
    .expect("out-of-process metadata");
    assert_eq!(
        graph
            .inputs("feature/model.gltf")
            .expect("inventoried")
            .iter()
            .map(|input| input.path.as_str())
            .collect::<Vec<_>>(),
        ["color.png", "feature/mesh.bin", "feature/opacity.png"]
    );
    assert_eq!(
        graph.users("color.png").collect::<Vec<_>>(),
        ["feature/model.gltf"]
    );
    assert_eq!(
        graph.users("feature/opacity.png").collect::<Vec<_>>(),
        ["feature/model.gltf"]
    );
    graph
        .verify(root.path(), &Cancellation::default())
        .expect("fresh");
    std::fs::remove_file(root.path().join("feature/mesh.bin")).expect("external removal");
    assert!(
        graph.verify(root.path(), &Cancellation::default()).is_err(),
        "missing companion must block mutation"
    );
    assert!(
        !root.path().join(".ara").exists(),
        "metadata does not publish/cook"
    );
}
