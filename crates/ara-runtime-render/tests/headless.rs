#![allow(clippy::unwrap_used)]
//! The demo scene rendered headless: labelled captures, motion between frames,
//! CPU-skinned Fox, live light edits and validation-clean dynamic uploads.
use ara_asset::{CachedImport, Cancellation, ImportContext, ImportInput, TargetPlatform};
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
    // The Fox's `.animation` controller documents.
    for entry in std::fs::read_dir(demo().join("anim")).unwrap() {
        let path = entry.unwrap().path();
        let relative = format!("anim/{}", path.file_name().unwrap().to_string_lossy());
        let text = std::fs::read_to_string(&path).unwrap();
        let doc = ara_doc::read(&text, &relative, &schema, None).doc.unwrap();
        workspace.insert(relative, doc);
    }
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/presenter-demo");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let mut presenter = Presenter::new([480, 270], Vec::new(), root.join("gpu"));
    let first = presenter
        .capture(&frame, [480, 270], ViewMode::Lit, Some(40), 1)
        .unwrap();
    first.write(&root.join("demo-start.png")).unwrap();
    assert!(presenter.stats().rebuilt);
    assert!(presenter.renderer().unwrap().is_native());
    assert!(
        !presenter
            .renderer()
            .unwrap()
            .gpu()
            .context()
            .info
            .ray_tracing
    );
    assert_eq!(
        presenter
            .renderer()
            .unwrap()
            .native_geometry_stats()
            .unwrap()
            .blas,
        0
    );
    if presenter
        .renderer()
        .unwrap()
        .gpu()
        .context()
        .info
        .ray_tracing
    {
        assert_eq!(
            presenter.quality(),
            Some(ara_runtime_render::QualityPreset::High)
        );
    }
    assert!(
        first.legend.iter().any(|m| m.name.contains("Fox")),
        "{:?}",
        first.legend
    );
    assert!(first.legend.len() >= 8, "{} marks", first.legend.len());
    let repeated = presenter
        .capture(&frame, [480, 270], ViewMode::Lit, Some(40), 1)
        .unwrap();
    repeated.write(&root.join("demo-repeated.png")).unwrap();
    let maximum = repeated
        .image
        .as_raw()
        .iter()
        .zip(first.image.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    // Allow one UNORM rounding step, without tolerating temporal/residency drift.
    assert!(
        maximum <= 1,
        "capture must not borrow live temporal history: maximum channel difference {maximum}"
    );

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
    if presenter
        .renderer()
        .unwrap()
        .gpu()
        .context()
        .info
        .timestamp_clock
        .is_some()
    {
        let span = stats.gpu_span_ms.expect("calibrated whole presenter span");
        assert!(span > 0.);
        let interval = presenter.renderer().unwrap().gpu().timing_window().unwrap();
        assert_eq!(span, interval.milliseconds());
        for pass in presenter.pass_timings() {
            assert!(
                pass.start_ms >= interval.start_ms && pass.end_ms <= interval.end_ms,
                "view pass must lie inside the prepare-through-publication span: {pass:?} {interval:?}"
            );
        }
    }
    if presenter.quality() == Some(ara_runtime_render::QualityPreset::High) {
        let costs = stats
            .gpu
            .expect("native frame exposes timestamp components");
        assert!(costs.raster_ms > 0. && costs.fidelity_ms == 0. && costs.publication_ms > 0.);
        assert!(
            costs.effects_ms > 0.,
            "demo campfire must be accounted: {costs:?}"
        );
        assert!(
            (costs.accounted_ms() - stats.gpu_ms).abs() < 1e-9,
            "native GPU components must account for the same frame: {stats:?}"
        );
    }
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
    let completed = presenter.output();
    let geometry = std::mem::replace(
        &mut frame.draws[0].geometry,
        ara_runtime::extract::Geometry::Shape {
            shape: ara_runtime::components::MeshShape::Box,
            size: [f32::NAN, 1., 1.],
        },
    );
    presenter.present(&frame).unwrap();
    assert!(presenter.presentation_error().is_some());
    assert_eq!(
        presenter.output(),
        completed,
        "failed content must retain the published image"
    );
    frame.draws[0].geometry = geometry;
    presenter.present(&frame).unwrap();
    assert!(presenter.presentation_error().is_none());
    if presenter.quality() == Some(ara_runtime_render::QualityPreset::High) {
        presenter.resize([640, 360]).unwrap();
        presenter.present(&frame).unwrap();
        assert!(presenter.presentation_error().is_none());
        assert_eq!(presenter.output_size(), [640, 360]);
        presenter
            .capture(&frame, [480, 270], ViewMode::Lit, None, 1)
            .unwrap();
        assert_eq!(presenter.size(), [640, 360]);
        presenter.present(&frame).unwrap();
        assert_eq!(presenter.output_size(), [640, 360]);
        presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
            ara_runtime_render::QualityPreset::Medium,
        ));
        presenter.present(&frame).unwrap();
        assert!(presenter.presentation_error().is_none());
        assert_eq!(
            presenter.quality(),
            Some(ara_runtime_render::QualityPreset::Medium)
        );
        frame.look.sky = Some(Default::default());
        presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
            ara_runtime_render::QualityPreset::High,
        ));
        presenter.present(&frame).unwrap();
        assert!(presenter.presentation_error().is_none());
        assert_eq!(
            presenter.quality(),
            Some(ara_runtime_render::QualityPreset::High)
        );
        // A new streamed identity replaces geometry without duplicating view targets.
        let mut arrival = frame.draws[0].clone();
        arrival.entity = 100_000;
        arrival.address = "streamed-arrival".into();
        frame.draws.push(arrival);
        presenter.present(&frame).unwrap();
        assert!(presenter.presentation_error().is_none());
        assert!(presenter.stats().rebuilt);
        frame.draws.pop();
        presenter.present(&frame).unwrap();
        assert!(presenter.presentation_error().is_none());
        assert!(presenter.stats().rebuilt);
        eprintln!("Native presenter warm frame: {:?}", presenter.stats());
    }
    presenter.renderer().unwrap().gpu().assert_clean().unwrap();
}
