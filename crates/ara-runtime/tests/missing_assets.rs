//! Broken imports keep the runtime playable with visible placeholder geometry.
#![allow(clippy::unwrap_used)]
use ara_runtime::{
    Runtime, RuntimeConfig,
    assets::MemoryAssets,
    extract::{Geometry, RenderFrame},
};
use std::path::Path;

#[test]
fn missing_model_keeps_simulation_and_placeholder_alive() {
    let schema = ara_runtime::schema(&[]).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-player/demo/levels/demo.scene");
    let read = ara_doc::read(
        &std::fs::read_to_string(path).unwrap(),
        "levels/demo.scene",
        &schema,
        None,
    );
    let doc = read.doc.unwrap();
    let id = doc.id.clone();
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert("levels/demo.scene", doc);
    let mut runtime = Runtime::new(
        schema,
        workspace,
        id,
        Box::new(MemoryAssets::default()),
        RuntimeConfig::default(),
        Vec::new(),
    )
    .unwrap();
    assert!(
        runtime
            .asset_errors()
            .iter()
            .any(|e| e.contains("ARA7310") && e.contains("as_fox"))
    );
    runtime.step(2, &[]).unwrap();
    let mut frame = RenderFrame::default();
    runtime.extract(&mut frame);
    let fox = frame.draws.iter().find(|d| d.address == "e_fox").unwrap();
    assert!(matches!(fox.geometry, Geometry::Shape { .. }));
    assert_eq!(fox.color, [1.0, 0.0, 1.0, 1.0]);
}
