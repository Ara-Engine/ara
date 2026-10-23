#![allow(clippy::unwrap_used)]
//! The demo scene rendered headless: labelled captures, motion between frames,
//! CPU-skinned Fox, live light edits and validation-clean dynamic uploads.
use ara_asset::{CachedImport, Cancellation, ImportContext, ImportInput, Importer, TargetPlatform};
use ara_doc::Workspace;
use ara_input::{ActionValue, Injection, PlayerId};
use ara_render::capture::{ViewMode, compare};
use ara_runtime::{
    Runtime, RuntimeConfig, assets::MemoryAssets, extract::RenderFrame, live::set_edit,
};
use ara_runtime_render::Presenter;
use ara_sim::StepInput;
use std::path::Path;

fn demo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../ara-player/demo"))
}

fn runtime() -> Runtime {
    let registry = ara_importers::registry().unwrap();
    let input = ImportInput {
        preset: ara_asset::CookPreset::Fast,
        thread_budget: 4,
        source_path: "assets/fox/Fox.glb".into(),
        source: std::fs::read(demo().join("assets/fox/Fox.glb")).unwrap(),
        importer: "ara.gltf".into(),
        version: registry.get("ara.gltf").unwrap().version().into(),
        settings: serde_json::Value::Null,
        target: TargetPlatform::Windows,
        dependencies: Default::default(),
        asset_dependencies: Default::default(),
    };
    let cancellation = Cancellation::default();
    let mut progress = |_| {};
    let mut context = ImportContext {
        metrics: None,
        cancellation: &cancellation,
        progress: &mut progress,
    };
    let output = registry.import(&input, &mut context).unwrap();
    let fox = CachedImport {
        key: input.key().unwrap(),
        output,
    };
    let schema = ara_runtime::schema(&[]).unwrap();
    let text = std::fs::read_to_string(demo().join("levels/demo.scene")).unwrap();
    let doc = ara_doc::read(&text, "levels/demo.scene", &schema, None)
        .doc
        .unwrap();
    let scene = doc.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("levels/demo.scene", doc);
    Runtime::new(
        schema,
        workspace,
        scene,
        Box::new(MemoryAssets([("as_fox".to_owned(), fox)].into())),
        RuntimeConfig::default(),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn demo_renders_moves_skins_and_relights_headless() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/runtime-render");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let mut presenter = Presenter::new([480, 270], Vec::new(), root.join("gpu"));
    let first = presenter
        .capture(&frame, [480, 270], ViewMode::Lit, Some(40), 1)
        .unwrap();
    first.write(&root.join("demo-start.png")).unwrap();
    assert!(presenter.stats().rebuilt);
    assert!(
        first.legend.iter().any(|m| m.name.contains("Fox")),
        "{:?}",
        first.legend
    );
    assert!(first.legend.len() >= 8, "{} marks", first.legend.len());

    let walk = StepInput {
        after_ticks: 0,
        input: Injection {
            player: PlayerId(1),
            action: "Move".into(),
            value: ActionValue::Axis2([0.3, 1.0]),
            hold_ticks: 90,
        },
    };
    rt.step(90, &[walk]).unwrap();
    rt.extract(&mut frame);
    presenter.present(&frame).unwrap();
    let stats = presenter.stats();
    assert!(!stats.rebuilt && stats.updated >= 5, "{stats:?}");
    let moved = presenter
        .capture(&frame, [480, 270], ViewMode::Lit, Some(40), 1)
        .unwrap();
    moved.write(&root.join("demo-walked.png")).unwrap();
    assert!(!compare(&moved.image, &first.image, 0.005).unwrap().passed);

    let scene = rt.play().session().projection().scene().clone();
    let edit = set_edit(
        &scene,
        "e_lamp1",
        "Light.color",
        Some(serde_json::json!([0.1, 1.0, 0.2, 1])),
    )
    .unwrap();
    rt.patch_edits(vec![edit]).unwrap();
    rt.step(1, &[]).unwrap();
    rt.extract(&mut frame);
    let relit = presenter
        .capture(&frame, [480, 270], ViewMode::Lit, Some(40), 1)
        .unwrap();
    assert!(
        !presenter.stats().rebuilt,
        "a light edit streams, it does not rebuild"
    );
    assert_ne!(relit.image, moved.image);
    presenter.renderer().unwrap().gpu().assert_clean().unwrap();
}
