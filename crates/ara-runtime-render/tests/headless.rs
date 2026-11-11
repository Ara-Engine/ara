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

fn demo() -> std::path::PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../ara-player/demo"))
        .join(ara_doc::AUTHORING_ROOT)
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
    // The presentation device keeps optional ray tracing, but without the opt-in
    // nothing traces rays, so the native geometry pool builds no BLASes.
    assert_eq!(
        presenter
            .renderer()
            .unwrap()
            .native_geometry_stats()
            .unwrap()
            .blas,
        0
    );
    // Auto picks the device's raster tier (by device-local memory: a shared-memory
    // integrated GPU stays at Low); ray tracing never lowers it.
    let capabilities =
        ara_render_fidelity::presets::Capabilities::of(presenter.renderer().unwrap().gpu());
    assert_eq!(
        presenter.quality(),
        Some(ara_runtime_render::QualityPreset::detect(&capabilities))
    );
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
        .info()
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
    // Fail the feature after the view has submitted GPU work. The batching
    // boundary must drain it, retain publication and allow a corrected retry.
    let last_good = presenter.output();
    let descriptors = presenter.renderer().unwrap().gpu().descriptor_usage();
    let scale = frame.effects[0].scale;
    frame.effects[0].scale = 0.;
    for _ in 0..8 {
        presenter.present(&frame).unwrap();
        assert!(presenter.presentation_error().is_some());
        assert_eq!(presenter.output(), last_good);
        assert_eq!(
            presenter.renderer().unwrap().gpu().descriptor_usage(),
            descriptors
        );
    }
    frame.effects[0].scale = scale;
    presenter.present(&frame).unwrap();
    assert!(presenter.presentation_error().is_none());
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

/// Frames in flight: a present returns once submitted, reporting the previous
/// frame's GPU costs and pass timings; turning it off drains the last frame.
#[test]
fn frames_in_flight_report_the_previous_frame() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/frames-in-flight");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let mut presenter = Presenter::new([480, 270], Vec::new(), root.join("gpu"));
    presenter.present(&frame).unwrap();
    assert!(
        presenter.stats().gpu.is_some(),
        "a synchronous present completes"
    );
    presenter.set_frames_in_flight(true).unwrap();
    presenter.present(&frame).unwrap();
    assert!(
        presenter.stats().gpu.is_none(),
        "the first frame in flight has no completed predecessor"
    );
    presenter.present(&frame).unwrap();
    let stats = presenter.stats();
    assert!(stats.gpu.is_some_and(|c| c.raster_ms > 0.), "{stats:?}");
    assert!(stats.gpu_span_ms.is_some(), "{stats:?}");
    assert!(!presenter.pass_timings().is_empty());
    presenter.set_frames_in_flight(false).unwrap();
    assert!(presenter.stats().gpu.is_some(), "turning it off drains");
    presenter.renderer().unwrap().gpu().assert_clean().unwrap();
}

#[cfg(feature = "fidelity-lab")]
#[test]
fn lab_readbacks_drain_frames_and_keep_overlap_enabled() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/lab-readback");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let mut presenter = Presenter::new([480, 270], Vec::new(), root.join("gpu"));
    presenter.present(&frame).unwrap();
    presenter.set_frames_in_flight(true).unwrap();

    presenter.present(&frame).unwrap();
    assert!(presenter.stats().gpu.is_none());
    let (size, radiance) = presenter.lab_radiance().unwrap();
    assert_eq!(radiance.len(), size[0] as usize * size[1] as usize);
    assert!(radiance.iter().flatten().all(|v| v.is_finite()));
    assert!(presenter.stats().gpu.is_some(), "radiance drains the frame");

    presenter.present(&frame).unwrap();
    assert!(presenter.stats().gpu.is_none(), "overlap remains enabled");
    let display = presenter.lab_display(&frame).unwrap();
    assert_eq!(display.dimensions(), (480, 270));
    assert!(presenter.stats().gpu.is_some(), "display drains the frame");

    presenter.present(&frame).unwrap();
    assert!(
        presenter.stats().gpu.is_none(),
        "readbacks preserve overlap"
    );
    presenter.set_frames_in_flight(false).unwrap();
    presenter.renderer().unwrap().gpu().assert_clean().unwrap();
}

/// Rendering Debug on the native path Play uses: shadows off and AO off each change
/// the High image, and restoring the defaults restores it. Captures go to
/// `target/renderer-unification/debug-switches` for review.
#[test]
fn debug_switches_change_the_native_high_image() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/debug-switches");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let mut presenter = Presenter::new([480, 270], Vec::new(), root.join("gpu"));
    presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
        ara_runtime_render::QualityPreset::High,
    ));
    let on = ara_render::RenderDebug::default();
    let mut shot = |debug: ara_render::RenderDebug, name: &str| {
        presenter.set_debug(debug);
        let capture = presenter
            .capture(&frame, [480, 270], ViewMode::Lit, None, 1)
            .unwrap();
        capture.write(&root.join(format!("{name}.png"))).unwrap();
        capture.image
    };
    let lit = shot(on, "default");
    let unshadowed = shot(
        ara_render::RenderDebug {
            shadows: false,
            ..on
        },
        "no-shadows",
    );
    let unoccluded = shot(
        ara_render::RenderDebug {
            ambient_occlusion: false,
            ..on
        },
        "no-ao",
    );
    let again = shot(on, "default-again");
    // Where AO darkens: the lit image over the unoccluded one (white = untouched).
    let factor = image::RgbaImage::from_fn(lit.width(), lit.height(), |x, y| {
        let (a, b) = (lit.get_pixel(x, y), unoccluded.get_pixel(x, y));
        let ratio = (0..3)
            .map(|c| f32::from(a[c]) / f32::from(b[c]).max(1.))
            .sum::<f32>()
            / 3.;
        // Five times the darkening, so faint false occlusion is visible.
        let v = ((1. - (1. - ratio.min(1.)) * 5.).max(0.) * 255.) as u8;
        image::Rgba([v, v, v, 255])
    });
    factor.save(root.join("ao-factor.png")).unwrap();
    assert!(
        !compare(&unshadowed, &lit, 0.002).unwrap().passed,
        "shadows off changed nothing"
    );
    assert!(
        !compare(&unoccluded, &lit, 0.002).unwrap().passed,
        "AO off changed nothing"
    );
    assert!(
        compare(&again, &lit, 0.002).unwrap().passed,
        "defaults did not restore the image"
    );
}

/// Temporal stability on the native High path with a still camera: consecutive live
/// frames (jitter and the temporal resolve running) barely change once the history
/// has converged. Reports the mean and worst 16x16-block luminance change, with and
/// without the jitter; frames go to `target/renderer-unification/temporal`.
#[test]
#[ignore = "temporal measurement: run explicitly while working on the resolve"]
fn still_camera_frames_hold_still() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/renderer-unification/temporal");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let size = [960, 540];
    let mut presenter = Presenter::new(size, Vec::new(), root.join("gpu"));
    presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
        ara_runtime_render::QualityPreset::High,
    ));
    let luma = |image: &image::RgbaImage| -> Vec<f32> {
        image
            .pixels()
            .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
            .collect()
    };
    for (name, debug) in [
        ("jitter", ara_render::RenderDebug::default()),
        (
            "still",
            ara_render::RenderDebug {
                temporal_jitter: false,
                ..Default::default()
            },
        ),
        (
            "jitter-full",
            ara_render::RenderDebug {
                full_resolution: true,
                ..Default::default()
            },
        ),
    ] {
        presenter.set_debug(debug);
        for _ in 0..64 {
            presenter.present(&frame).unwrap();
        }
        let request = ara_render::capture::CaptureRequest {
            // Readback only: the completed frame keeps its own camera.
            camera: ara_render::scene::CameraSpec::Orthographic {
                centre: Default::default(),
                world_size: Default::default(),
            },
            size,
            mode: ViewMode::Lit,
            deterministic: false,
            exposure: 1.,
            settle_frames: 0,
            tick: 0,
            seed: 0,
            marks: None,
        };
        let mut previous: Option<Vec<f32>> = None;
        let (mut mean, mut worst) = (0f64, 0f32);
        for k in 0..8 {
            presenter.present(&frame).unwrap();
            let image = presenter
                .renderer_mut()
                .unwrap()
                .capture_completed(&request)
                .unwrap()
                .image;
            image.save(root.join(format!("{name}-{k}.png"))).unwrap();
            let current = luma(&image);
            if let Some(previous) = &previous {
                let diff: Vec<f32> = current
                    .iter()
                    .zip(previous)
                    .map(|(a, b)| (a - b).abs())
                    .collect();
                mean += diff.iter().map(|&d| f64::from(d)).sum::<f64>() / diff.len() as f64;
                for by in 0..size[1] as usize / 16 {
                    for bx in 0..size[0] as usize / 16 {
                        let mut block = 0f32;
                        for y in 0..16 {
                            for x in 0..16 {
                                block += diff[(by * 16 + y) * size[0] as usize + bx * 16 + x];
                            }
                        }
                        worst = worst.max(block / 256.);
                    }
                }
            }
            previous = Some(current);
        }
        eprintln!(
            "{name}: mean change {:.3}, worst 16x16 block {:.2} (8-bit luma)",
            mean / 7.,
            worst
        );
    }
}
