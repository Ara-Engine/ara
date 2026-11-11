#![allow(clippy::unwrap_used)]
//! Soft shadow penumbrae (ara-render `shaders/look-shadow.slangh`) against the
//! path-traced reference: a thin plate's straight edge over a floor, lit from straight
//! above by a rect, a disc, a sphere light or the sun, seen from a camera under the
//! plate looking down. Each renderer's shadow is the ratio of the floor with and
//! without the plate (so falloff and shading cancel), and its 10-90% width across the
//! edge is compared with the reference's (and the analytic width). Raster runs with
//! GI, AO and reflections off and the reference with one scattering event, both at
//! one locked EV100 through the linear part of the neutral tone curve. Images go to
//! `target/soft-shadows/`.
//!
//! `penumbrae_match_the_path_traced_reference` checks a rect and the sun at a wide
//! angle; `penumbra_table` (ignored: `cargo test -p ara-runtime-render --test
//! soft_shadows -- --ignored --nocapture`) prints every case at Medium and High.
use ara_doc::Workspace;
use ara_frame::{capture::{Capture, ViewMode}};
use ara_render::{ReferenceTrace, RenderDebug};
use ara_runtime::{Runtime, RuntimeConfig, assets::MemoryAssets, extract::RenderFrame};
use ara_runtime_render::{Presenter, QualityChoice, QualityPreset};
use std::path::{Path, PathBuf};

/// One penumbra case: a light `height` metres above the floor over the plate's edge
/// (at x = 0, the plate covering x < 0) `plate` metres up, seen from `camera` metres
/// up with a `fov`-degree view.
#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    light: Light,
    height: f32,
    plate: f32,
    camera: f32,
    fov: f32,
}

#[derive(Clone, Copy)]
enum Light {
    /// Width along x (and depth), metres.
    Rect(f32),
    /// Radius, metres.
    Disc(f32),
    Sphere(f32),
    /// Angular diameter, degrees.
    Sun(f32),
}

impl Case {
    /// The analytic 10-90% width (m): a rect's linear ramp, a disc's or sphere's
    /// (and the sun disc's) circular-segment profile.
    fn expected(&self) -> f32 {
        let scale = self.plate / (self.height - self.plate);
        // A disc's area share past a chord at t radii from its centre reaches 10%
        // and 90% at t = +-0.6870.
        match self.light {
            Light::Rect(w) => 0.8 * w * scale,
            Light::Disc(r) | Light::Sphere(r) => 2. * 0.687 * r * scale,
            Light::Sun(degrees) => 2. * 0.687 * self.plate * (degrees.to_radians() * 0.5).tan(),
        }
    }

    fn scene(&self, plate: bool) -> String {
        let light = match self.light {
            Light::Rect(w) => format!(
                r#""Light": {{"kind": "rect", "intensity": 300, "range": 20, "sourceWidth": {w}, "sourceHeight": {w}, "barnDoorLength": 0}},
      "Transform": {{"position": [0, {h}, 0], "rotation": [-90, 0, 0]}}"#,
                h = self.height
            ),
            Light::Disc(r) => format!(
                r#""Light": {{"kind": "disc", "intensity": 300, "range": 20, "sourceRadius": {r}}},
      "Transform": {{"position": [0, {h}, 0], "rotation": [-90, 0, 0]}}"#,
                h = self.height
            ),
            Light::Sphere(r) => format!(
                r#""Light": {{"kind": "point", "intensity": 1200, "range": 20, "sourceRadius": {r}}},
      "Transform": {{"position": [0, {h}, 0]}}"#,
                h = self.height
            ),
            Light::Sun(degrees) => format!(
                r#""Light": {{"kind": "directional", "intensity": 6, "sourceAngle": {degrees}}},
      "Transform": {{"position": [0, 10, 0], "rotation": [-90, 0, 0]}}"#
            ),
        };
        let plate = if plate {
            format!(
                r#",
    "e_plate": {{"name": "Plate", "order": "a4",
      "MeshRenderer": {{"shape": "box", "size": [6, 0.02, 12], "color": [0.5, 0.5, 0.5, 1], "roughness": 1}},
      "Transform": {{"position": [-3, {}, 0]}}}}"#,
                self.plate + 0.01
            )
        } else {
            String::new()
        };
        format!(
            r#"{{
  "$schema": "ara://schema/scene/1",
  "id": "sc_softshadowtest",
  "records": {{
    "e_camera": {{"name": "Camera", "order": "a0", "Camera": {{"fov": {fov}}},
      "Transform": {{"position": [0, {camera}, 0], "rotation": [-90, 0, 0]}}}},
    "e_env": {{"name": "Environment", "order": "a1",
      "Environment": {{"sky": [0, 0, 0, 1], "horizon": [0, 0, 0, 1], "ground": [0, 0, 0, 1], "intensity": 0}}}},
    "e_post": {{"name": "Post", "order": "a2", "PostProcessVolume": {{"overrides": [
      {{"module": "toneMapping", "toneMapping": "neutral"}}, {{"module": "bloom", "bloom": 0}},
      {{"module": "vignette", "vignette": 0}}]}}}},
    "e_floor": {{"name": "Floor", "order": "a3",
      "MeshRenderer": {{"shape": "plane", "size": [40, 1, 40], "color": [0.5, 0.5, 0.5, 1], "roughness": 1}}}},
    "e_light": {{"name": "Light", "order": "a5", {light}}}{plate}
  }}
}}"#,
            fov = self.fov,
            camera = self.camera,
        )
    }
}

fn runtime(scene: &str) -> Runtime {
    let schema = ara_runtime::schema(&[]).unwrap();
    let doc = ara_doc::read(scene, "levels/soft.scene", &schema, None)
        .doc
        .unwrap();
    let id = doc.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("levels/soft.scene", doc);
    Runtime::new(
        schema,
        workspace,
        id,
        Box::new(MemoryAssets::default()),
        RuntimeConfig::default(),
        Vec::new(),
    )
    .unwrap()
}

/// Scene-linear value of a display byte: sRGB decoded, then the neutral curve
/// undone (linear minus 0.04 between 0.08 and 0.76, 6.25 x^2 below).
fn linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.;
    let d = if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    };
    if d < 0.04 {
        (d / 6.25).sqrt()
    } else {
        d + 0.04
    }
}

/// Mean scene-linear luminance per column over the middle fifth of the rows.
fn columns(image: &image::RgbaImage) -> Vec<f32> {
    let (w, h) = (image.width(), image.height());
    (0..w)
        .map(|x| {
            let rows = h * 2 / 5..h * 3 / 5;
            let n = rows.len() as f32;
            rows.map(|y| {
                let p = image.get_pixel(x, y);
                0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2])
            })
            .sum::<f32>()
                / n
        })
        .collect()
}

/// The plate's shadow: per column, the floor with the plate over the floor without.
fn visibility(with: &Capture, without: &Capture) -> Vec<f32> {
    columns(&with.image)
        .iter()
        .zip(columns(&without.image))
        .map(|(a, b)| a / b.max(1e-4))
        .collect()
}

/// The 10-90% width of a profile across the edge, in columns (linear between columns).
fn width(profile: &[f32]) -> f32 {
    let mut profile = profile.to_vec();
    if profile.first() > profile.last() {
        profile.reverse();
    }
    let crossing = |level: f32| {
        profile
            .windows(2)
            .enumerate()
            .find(|(_, w)| w[0] < level && w[1] >= level)
            .map(|(i, w)| i as f32 + (level - w[0]) / (w[1] - w[0]))
    };
    match (crossing(0.1), crossing(0.9)) {
        (Some(a), Some(b)) => b - a,
        _ => f32::NAN,
    }
}

/// Raster (`preset`) and reference visibility profiles and 10-90% widths in metres
/// for `case`; `None` without hardware ray tracing.
fn measure(case: &Case, preset: QualityPreset, root: &Path) -> Option<[f32; 2]> {
    let size = [640, 360];
    let direct = RenderDebug {
        exposure: Some(1.),
        global_illumination: false,
        ambient_occlusion: false,
        screen_reflections: false,
        reflection_captures: false,
        full_resolution: true,
        ..RenderDebug::default()
    };
    let reference = RenderDebug {
        reference: Some(ReferenceTrace {
            samples: 512,
            bounces: 1,
        }),
        ..direct
    };
    let mut images: Vec<Capture> = Vec::new();
    for debug in [direct, reference] {
        for plate in [true, false] {
            let mut rt = runtime(&case.scene(plate));
            let mut frame = RenderFrame::default();
            rt.extract(&mut frame);
            let mut presenter = Presenter::new(size, Vec::new(), root.join("gpu"));
            presenter.set_quality(QualityChoice::Preset(preset));
            presenter.set_debug(debug);
            match presenter.capture(&frame, size, ViewMode::Lit, None, 8) {
                Ok(capture) => images.push(capture),
                Err(error) if error.to_string().contains("ARA6009") => {
                    eprintln!("skipped: the reference needs hardware ray tracing ({error})");
                    return None;
                }
                Err(error) => panic!("{error}"),
            }
        }
    }
    let tag = format!("{}_{preset:?}", case.name).to_lowercase();
    for (image, name) in images
        .iter()
        .zip(["raster", "raster_open", "reference", "reference_open"])
    {
        image
            .write(&root.join(format!("{tag}_{name}.png")))
            .unwrap();
    }
    // Metres per column on the floor (the view's horizontal half-angle from the
    // vertical fov and the aspect).
    let half = (case.fov.to_radians() * 0.5).tan() * size[0] as f32 / size[1] as f32;
    let metres = 2. * case.camera * half / size[0] as f32;
    let raster = visibility(&images[0], &images[1]);
    let traced = visibility(&images[2], &images[3]);
    let csv: String = raster
        .iter()
        .zip(&traced)
        .enumerate()
        .map(|(i, (r, t))| {
            format!(
                "{:.4},{r:.4},{t:.4}\n",
                (i as f32 + 0.5) * metres - case.camera * half
            )
        })
        .collect();
    std::fs::write(
        root.join(format!("{tag}_profile.csv")),
        format!("x,raster,reference\n{csv}"),
    )
    .unwrap();
    Some([width(&raster) * metres, width(&traced) * metres])
}

const CASES: [Case; 7] = [
    Case {
        name: "rect",
        light: Light::Rect(1.2),
        height: 4.,
        plate: 1.,
        camera: 0.8,
        fov: 50.,
    },
    Case {
        name: "rect_low",
        light: Light::Rect(1.2),
        height: 4.,
        plate: 0.3,
        camera: 0.24,
        fov: 50.,
    },
    Case {
        name: "disc",
        light: Light::Disc(0.6),
        height: 4.,
        plate: 1.,
        camera: 0.8,
        fov: 50.,
    },
    Case {
        name: "sphere",
        light: Light::Sphere(0.6),
        height: 4.,
        plate: 1.,
        camera: 0.8,
        fov: 50.,
    },
    Case {
        name: "sun",
        light: Light::Sun(0.5357),
        height: 10.,
        plate: 3.,
        camera: 0.15,
        fov: 40.,
    },
    Case {
        name: "sun_wide",
        light: Light::Sun(10.),
        height: 10.,
        plate: 1.,
        camera: 0.6,
        fov: 40.,
    },
    Case {
        name: "sun_tall",
        light: Light::Sun(0.5357),
        height: 10.,
        plate: 8.,
        camera: 0.25,
        fov: 40.,
    },
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/soft-shadows")
}

#[test]
fn penumbrae_match_the_path_traced_reference() {
    let root = root();
    let mut failures = Vec::new();
    for case in [CASES[0], CASES[5]] {
        let Some([raster, traced]) = measure(&case, QualityPreset::Medium, &root) else {
            return;
        };
        let error = (raster - traced).abs() / traced;
        eprintln!(
            "{}: raster {raster:.3} m, reference {traced:.3} m ({:.0}%), analytic {:.3} m",
            case.name,
            error * 100.,
            case.expected()
        );
        if error.is_nan() || error > 0.2 {
            failures.push(format!("{} {:.0}%", case.name, error * 100.));
        }
    }
    assert!(failures.is_empty(), "penumbra widths off: {failures:?}");
}

#[test]
#[ignore = "every case at two presets: a minute of path tracing"]
fn penumbra_table() {
    let root = root();
    eprintln!("case, preset: raster / reference / analytic 10-90% width (m)");
    for case in CASES {
        for preset in [QualityPreset::Medium, QualityPreset::High] {
            let Some([raster, traced]) = measure(&case, preset, &root) else {
                return;
            };
            eprintln!(
                "{}, {preset:?}: {raster:.4} / {traced:.4} / {:.4} ({:+.0}%)",
                case.name,
                case.expected(),
                (raster / traced - 1.) * 100.
            );
        }
    }
}
