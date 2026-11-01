#![allow(clippy::unwrap_used)]
//! A rect light's raster shading (LTC diffuse and specular) against the path-traced
//! reference on a simple scene: an overhead rect over a floor, a rough and a glossy
//! ball. Raster runs with probe GI, AO and traced reflections off and the reference
//! with one scattering event, so both show direct light only, at one locked EV100.
//! Images go to `target/area-lights/`.
use ara_doc::Workspace;
use ara_render::{ReferenceTrace, RenderDebug, capture::ViewMode};
use ara_runtime::{Runtime, RuntimeConfig, assets::MemoryAssets, extract::RenderFrame};
use ara_runtime_render::Presenter;
use std::path::Path;

const SCENE: &str = r#"{
  "$schema": "ara://schema/scene/1",
  "id": "sc_arealighttest1",
  "records": {
    "e_camera": {"name": "Camera", "order": "a0", "Camera": {"fov": 50},
      "Transform": {"position": [0, 3.2, 4.2], "rotation": [-38, 0, 0]}},
    "e_env": {"name": "Environment", "order": "a1",
      "Environment": {"sky": [0, 0, 0, 1], "horizon": [0, 0, 0, 1], "ground": [0, 0, 0, 1], "intensity": 0}},
    "e_floor": {"name": "Floor", "order": "a2",
      "MeshRenderer": {"shape": "plane", "size": [10, 1, 10], "color": [0.5, 0.5, 0.5, 1], "roughness": 0.45}},
    "e_rough": {"name": "Rough ball", "order": "a3",
      "MeshRenderer": {"shape": "sphere", "size": [0.8, 0.8, 0.8], "color": [0.8, 0.8, 0.8, 1], "roughness": 0.85},
      "Transform": {"position": [-0.9, 0.4, 0]}},
    "e_glossy": {"name": "Glossy ball", "order": "a4",
      "MeshRenderer": {"shape": "sphere", "size": [0.8, 0.8, 0.8], "color": [0.9, 0.9, 0.9, 1], "roughness": 0.35, "metallic": 1},
      "Transform": {"position": [0.9, 0.4, 0]}},
    "e_rect": {"name": "Rect light", "order": "a5",
      "Light": {"kind": "rect", "intensity": 2500, "range": 15, "sourceWidth": 1.6, "sourceHeight": 0.8, "barnDoorLength": 0},
      "Transform": {"position": [0, 2.4, -0.4], "rotation": [-90, 0, 0]}}
  }
}"#;

fn runtime() -> Runtime {
    let schema = ara_runtime::schema(&[]).unwrap();
    let doc = ara_doc::read(SCENE, "levels/area.scene", &schema, None)
        .doc
        .unwrap();
    let scene = doc.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("levels/area.scene", doc);
    Runtime::new(
        schema,
        workspace,
        scene,
        Box::new(MemoryAssets::default()),
        RuntimeConfig::default(),
        Vec::new(),
    )
    .unwrap()
}

/// Mean linear luminance (undoing sRGB) of `image` over the fractional box `[x0, y0, x1, y1]`.
fn mean(image: &image::RgbaImage, [x0, y0, x1, y1]: [f32; 4]) -> f32 {
    let (w, h) = (image.width() as f32, image.height() as f32);
    let (mut sum, mut count) = (0f32, 0f32);
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = image.get_pixel(x, y);
            let linear = |c: u8| {
                let c = f32::from(c) / 255.;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            sum += 0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2]);
            count += 1.;
        }
    }
    sum / count.max(1.)
}

#[test]
fn rect_light_matches_the_path_traced_reference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/area-lights");
    let mut rt = runtime();
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let size = [480, 270];
    let mut presenter = Presenter::new(size, Vec::new(), root.join("gpu"));
    presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
        ara_runtime_render::QualityPreset::High,
    ));
    let direct = RenderDebug {
        exposure: Some(3.),
        global_illumination: false,
        ambient_occlusion: false,
        screen_reflections: false,
        reflection_captures: false,
        full_resolution: true,
        ..RenderDebug::default()
    };
    presenter.set_debug(direct);
    let raster = presenter
        .capture(&frame, size, ViewMode::Lit, None, 8)
        .unwrap();
    raster.write(&root.join("raster.png")).unwrap();
    presenter.set_debug(RenderDebug {
        reference: Some(ReferenceTrace {
            samples: 1024,
            bounces: 1,
        }),
        ..direct
    });
    let reference = match presenter.capture(&frame, size, ViewMode::Lit, None, 8) {
        Ok(capture) => capture,
        Err(error) if error.to_string().contains("ARA6009") => {
            eprintln!("skipped: the reference needs hardware ray tracing ({error})");
            return;
        }
        Err(error) => panic!("{error}"),
    };
    reference.write(&root.join("reference.png")).unwrap();
    // Regions in the frame (fractions): lit floor left and right of the balls, in
    // front of them, the balls' upper halves, and the whole image. Measured 2026-11-01:
    // floor 0.5-1%, rough ball 3%, glossy ball 6% (its hard shadow terminator and the
    // lobe-averaged Fresnel), image 1%.
    let regions = [
        ("floor left", [0.05, 0.55, 0.25, 0.75], 0.03),
        ("floor right", [0.75, 0.55, 0.95, 0.75], 0.03),
        ("floor front", [0.3, 0.88, 0.7, 0.99], 0.03),
        ("rough ball", [0.3, 0.38, 0.42, 0.5], 0.06),
        ("glossy ball", [0.58, 0.38, 0.7, 0.5], 0.1),
        ("image", [0., 0., 1., 1.], 0.03),
    ];
    let mut failures = Vec::new();
    for (name, region, tolerance) in regions {
        let (r, p) = (mean(&raster.image, region), mean(&reference.image, region));
        let error = (r - p).abs() / p.max(1e-4);
        eprintln!(
            "{name}: raster {r:.4} reference {p:.4} ({:.1}%)",
            error * 100.
        );
        if error > tolerance {
            failures.push(format!(
                "{name} {:.1}% > {:.0}%",
                error * 100.,
                tolerance * 100.
            ));
        }
    }
    assert!(failures.is_empty(), "raster vs reference: {failures:?}");
}
