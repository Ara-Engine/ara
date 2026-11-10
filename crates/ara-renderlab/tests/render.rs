#![allow(clippy::unwrap_used)]
//! GPU check of the whole route: a hand-built frame (floor, box, glowing sphere, a
//! sun and a spot) goes through the presenter with Renderlab installed, on each
//! Renderlab path this machine's adapter supports. It checks that a still capture
//! and a live present both deliver a lit image of the scene on the presenter's
//! device, and that a moved object moves. Images go under the test's target
//! directory. A path the adapter cannot run is skipped with a note.
#![cfg(not(target_vendor = "apple"))]

use ara_render::capture::{CaptureRequest, ViewMode};
use ara_renderlab::{Options, PathChoice, Renderlab};
use ara_runtime::{
    components::{LightKind, MeshShape},
    extract::{CameraView, Draw, Geometry, LightView, RenderFrame},
};
use ara_runtime_render::{Presenter, fidelity::native::Content};
use glam34::{Mat4, Vec3};
use std::path::PathBuf;

const SIZE: [u32; 2] = [640, 360];

fn shape(entity: u64, shape: MeshShape, size: [f32; 3], at: Vec3, color: [f32; 4]) -> Draw {
    Draw {
        entity,
        address: format!("e_{entity}"),
        label: format!("shape {entity}"),
        geometry: Geometry::Shape { shape, size },
        world: Mat4::from_translation(at),
        color,
        roughness: 0.6,
        metallic: 0.,
        emissive: [0.; 3],
        emissive_factor: None,
        material: None,
        skin: None,
        casts_shadows: true,
        receives_decals: true,
        paint: None,
    }
}

/// A grey floor, a red box at `box_x` and a glowing sphere, under a sun and a spot.
fn frame(box_x: f32) -> RenderFrame {
    let mut frame = RenderFrame::default();
    // A daylight exposure (about EV100 14.3) for the 60,000 lux sun below.
    frame.environment.exposure = 4e-5;
    let eye = Vec3::new(0., 2.5, 7.);
    frame.camera = Some(CameraView {
        position: eye.to_array(),
        forward: (Vec3::new(0., 0.5, 0.) - eye).normalize().to_array(),
        fov: 50.,
    });
    frame.sun = Some((
        Vec3::new(0.4, 0.8, 0.3).normalize().to_array(),
        [60_000.; 3],
    ));
    frame.lights.push(LightView {
        kind: LightKind::Spot,
        position: [-3., 3., 2.],
        direction: Vec3::new(3., -3., -2.).normalize().to_array(),
        color: [1., 0.8, 0.6],
        lumens: 50_000.,
        range: 20.,
        cone: [20., 35.],
        ..Default::default()
    });
    frame.draws.push(shape(
        1,
        MeshShape::Plane,
        [20., 1., 20.],
        Vec3::ZERO,
        [0.5, 0.5, 0.5, 1.],
    ));
    frame.draws.push(shape(
        2,
        MeshShape::Box,
        [1., 1., 1.],
        Vec3::new(box_x, 0.5, 0.),
        [0.8, 0.1, 0.1, 1.],
    ));
    let mut lamp = shape(
        3,
        MeshShape::Sphere,
        [0.6, 0.6, 0.6],
        Vec3::new(1.8, 0.3, 1.),
        [1.; 4],
    );
    lamp.emissive = [60_000., 40_000., 12_000.];
    frame.draws.push(lamp);
    frame
}

/// Mean of `rgba`'s red minus its green and blue over the pixel columns `x`.
fn redness(rgba: &image::RgbaImage, x: std::ops::Range<u32>) -> f32 {
    let mut sum = 0.;
    for y in 0..rgba.height() {
        for x in x.clone() {
            let p = rgba.get_pixel(x, y).0;
            sum += f32::from(p[0]) - 0.5 * (f32::from(p[1]) + f32::from(p[2]));
        }
    }
    sum / (rgba.height() * (x.end - x.start)) as f32
}

fn check(path: PathChoice, name: &str) {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("renderlab");
    std::fs::create_dir_all(&out).unwrap();
    let renderlab = match Renderlab::new(&Options {
        path,
        pipeline_cache: Some(out.join("pipeline-cache")),
        ..Default::default()
    }) {
        Ok(renderlab) => renderlab,
        Err(error) => {
            println!("skipping {name}: {error:#}");
            return;
        }
    };
    let mut presenter = Presenter::new(SIZE, Vec::new(), out.join("gpu-cache"));
    presenter
        .set_external_renderer(Some(Box::new(renderlab)))
        .unwrap();
    let left = frame(-2.);
    let request = CaptureRequest {
        camera: ara_runtime_render::camera(&left).unwrap(),
        size: SIZE,
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 4,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let capture = presenter
        .capture_with(&left, &request, || Ok(Content::default()))
        .unwrap();
    capture
        .image
        .save(out.join(format!("{name}-left.png")))
        .unwrap();
    assert_eq!(capture.image.dimensions(), (SIZE[0], SIZE[1]));

    // Lit and metered: neither black nor blown out, and not one flat colour.
    let luma: Vec<f32> = capture
        .image
        .pixels()
        .map(|p| {
            0.2126 * f32::from(p.0[0]) + 0.7152 * f32::from(p.0[1]) + 0.0722 * f32::from(p.0[2])
        })
        .collect();
    let mean = luma.iter().sum::<f32>() / luma.len() as f32;
    let spread = luma.iter().map(|l| (l - mean).abs()).sum::<f32>() / luma.len() as f32;
    assert!(
        (40. ..215.).contains(&mean),
        "{name}: mean luminance {mean}"
    );
    assert!(spread > 8., "{name}: the image is nearly flat ({spread})");
    // The red box is left of centre.
    let half = SIZE[0] / 2;
    let (l, r) = (
        redness(&capture.image, 0..half),
        redness(&capture.image, half..SIZE[0]),
    );
    assert!(
        l > r + 1.,
        "{name}: red box expected on the left ({l} vs {r})"
    );
    // The capture reports what the external path measured.
    let timed: Vec<&str> = capture
        .metadata
        .timings
        .iter()
        .map(|t| t.pass.as_str())
        .collect();
    assert!(timed.contains(&"External.Handoff.Wall"), "{timed:?}");
    // With GPU timestamps, Renderlab's own passes are listed beside its frame span.
    let span = capture
        .metadata
        .timings
        .iter()
        .find(|t| t.pass == "External.Frame.Gpu");
    if span.is_some_and(|t| t.milliseconds > 0.) {
        assert!(timed.contains(&"External.Pass.Shading"), "{timed:?}");
    }

    // Live presents: the box moves right, and the image arrives on Ara's device.
    let right = frame(2.);
    for _ in 0..24 {
        presenter.present(&right).unwrap();
    }
    assert!(presenter.output().is_some());
    assert!(
        presenter
            .stats()
            .external_handoff_ms
            .is_some_and(|ms| ms > 0.)
    );
    assert_eq!(presenter.output_size(), SIZE);
    let moved = presenter
        .capture_with(&right, &request, || Ok(Content::default()))
        .unwrap();
    moved
        .image
        .save(out.join(format!("{name}-right.png")))
        .unwrap();
    let (l, r) = (
        redness(&moved.image, 0..half),
        redness(&moved.image, half..SIZE[0]),
    );
    assert!(
        r > l + 1.,
        "{name}: red box expected on the right ({l} vs {r})"
    );
}

/// One test, so the two paths never hold devices at the same time.
#[test]
fn renderlab_draws_a_frame_through_the_presenter_on_each_supported_path() {
    check(PathChoice::HardwareRt, "rt");
    check(PathChoice::NonRt, "nonrt");
}

/// An equirectangular HDR sky: blue above the horizon with a warm bright patch
/// toward +X, dark green below.
fn studio_sky() -> ara_runtime_render::fidelity::EnvironmentMap {
    let size = [256u32, 128u32];
    let mut pixels = Vec::with_capacity((size[0] * size[1]) as usize);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let above = y < size[1] / 2;
            // u = 0.5 is +X; the patch sits 30 to 50 degrees above the horizon there.
            let patch = (118..138).contains(&x) && (28..43).contains(&y);
            pixels.push(match (patch, above) {
                (true, _) => [60_000., 45_000., 30_000., 1.],
                (false, true) => [600., 1_200., 2_400., 1.],
                (false, false) => [120., 260., 90., 1.],
            });
        }
    }
    ara_runtime_render::fidelity::EnvironmentMap { size, pixels }
}

/// A content-supplied HDR sky is drawn behind the scene and lights it.
#[test]
fn an_hdr_environment_map_backs_and_lights_the_frame() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("renderlab");
    std::fs::create_dir_all(&out).unwrap();
    for (path, name) in [(PathChoice::HardwareRt, "rt"), (PathChoice::NonRt, "nonrt")] {
        let renderlab = match Renderlab::new(&Options {
            path,
            pipeline_cache: Some(out.join("pipeline-cache")),
            ..Default::default()
        }) {
            Ok(renderlab) => renderlab,
            Err(error) => {
                println!("skipping {name}: {error:#}");
                continue;
            }
        };
        let mut presenter = Presenter::new(SIZE, Vec::new(), out.join("gpu-cache"));
        presenter
            .set_external_renderer(Some(Box::new(renderlab)))
            .unwrap();
        // No sun and no spot: only the sky lights the floor, the box and the sphere.
        let mut frame = frame(-2.);
        frame.sun = None;
        frame.lights.clear();
        frame.draws[2].emissive = [0.; 3];
        frame.environment.exposure = 2e-4;
        let request = CaptureRequest {
            camera: ara_runtime_render::camera(&frame).unwrap(),
            size: SIZE,
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 1.,
            settle_frames: 4,
            tick: 0,
            seed: 0,
            marks: None,
        };
        let capture = presenter
            .capture_with(&frame, &request, || {
                Ok(Content {
                    environment: Some(studio_sky()),
                    ..Default::default()
                })
            })
            .unwrap();
        capture
            .image
            .save(out.join(format!("{name}-hdr-sky.png")))
            .unwrap();
        // The top rows look over the scene at the sky above the horizon: blue.
        let (mut red, mut blue) = (0f32, 0f32);
        for x in 0..SIZE[0] {
            for y in 0..SIZE[1] / 6 {
                let p = capture.image.get_pixel(x, y).0;
                red += f32::from(p[0]);
                blue += f32::from(p[2]);
            }
        }
        assert!(
            blue > red * 1.3,
            "{name}: sky rows are not blue ({red} vs {blue})"
        );
        // The floor in front is lit by that sky: neither black nor flat white.
        let floor = capture.image.get_pixel(SIZE[0] / 2, SIZE[1] - 20).0;
        assert!((20..250).contains(&floor[1]), "{name}: floor {floor:?}");
    }
}

/// 600 boxes, each its own mesh, in four colours: 601 mesh-material groups, more than
/// Renderlab's 256. Seen from above.
fn crowd() -> RenderFrame {
    let mut frame = frame(0.);
    frame.draws.truncate(1);
    frame.lights.clear();
    let eye = Vec3::new(0., 15., 9.);
    frame.camera = Some(CameraView {
        position: eye.to_array(),
        forward: (Vec3::ZERO - eye).normalize().to_array(),
        fov: 50.,
    });
    const COLOURS: [[f32; 4]; 4] = [
        [0.8, 0.05, 0.05, 1.],
        [0.05, 0.7, 0.05, 1.],
        [0.05, 0.1, 0.8, 1.],
        [0.8, 0.7, 0.05, 1.],
    ];
    for i in 0..600u32 {
        let at = Vec3::new(
            (i % 25) as f32 * 0.6 - 7.2,
            0.15,
            (i / 25) as f32 * 0.6 - 6.9,
        );
        // A size of its own gives every box its own mesh.
        let size = 0.3 + i as f32 * 1e-4;
        frame.draws.push(shape(
            10 + u64::from(i),
            MeshShape::Box,
            [size; 3],
            at,
            COLOURS[i as usize % 4],
        ));
    }
    frame
}

/// The dominant channel of a saturated pixel (0 red, 1 green, 2 blue, 3 yellow); None
/// for grey.
fn hue(p: [u8; 4]) -> Option<usize> {
    let [r, g, b, _] = p.map(i32::from);
    if r.max(g).max(b) - r.min(g).min(b) < 40 {
        return None;
    }
    Some(if r > 2 * b && g > 2 * b && (r - g).abs() * 3 < r.max(g) {
        3
    } else if r >= g && r >= b {
        0
    } else if g >= b {
        1
    } else {
        2
    })
}

/// Past the limit, static groups merge into batches instead of being left out: on
/// each Renderlab path, every box shows its own colour at the centre of its top.
#[test]
fn more_groups_than_the_limit_all_draw() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("renderlab");
    std::fs::create_dir_all(&out).unwrap();
    let frame = crowd();
    let request = CaptureRequest {
        camera: ara_runtime_render::camera(&frame).unwrap(),
        size: SIZE,
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 4,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let camera = frame.camera.unwrap();
    let eye = Vec3::from(camera.position);
    let view = glam34::camera::rh::view::look_to_mat4(eye, Vec3::from(camera.forward), Vec3::Y);
    let projection = glam34::camera::rh::proj::opengl::perspective(
        camera.fov.to_radians(),
        SIZE[0] as f32 / SIZE[1] as f32,
        0.1,
        100.,
    );
    for (path, name) in [(PathChoice::HardwareRt, "rt"), (PathChoice::NonRt, "nonrt")] {
        let renderlab = match Renderlab::new(&Options {
            path,
            pipeline_cache: Some(out.join("pipeline-cache")),
            ..Default::default()
        }) {
            Ok(renderlab) => renderlab,
            Err(error) => {
                println!("skipping {name}: {error:#}");
                continue;
            }
        };
        let mut presenter = Presenter::new(SIZE, Vec::new(), out.join("gpu-cache"));
        presenter
            .set_external_renderer(Some(Box::new(renderlab)))
            .unwrap();
        let capture = presenter
            .capture_with(&frame, &request, || Ok(Content::default()))
            .unwrap();
        capture
            .image
            .save(out.join(format!("crowd-{name}.png")))
            .unwrap();
        let mut wrong = Vec::new();
        for (i, draw) in frame.draws[1..].iter().enumerate() {
            let top = draw.world.transform_point3(Vec3::new(0., 0.15, 0.));
            let ndc = projection.project_point3(view.transform_point3(top));
            let x = ((ndc.x * 0.5 + 0.5) * SIZE[0] as f32) as u32;
            let y = ((0.5 - ndc.y * 0.5) * SIZE[1] as f32) as u32;
            let seen = hue(capture.image.get_pixel(x, y).0);
            if seen != Some(i % 4) {
                wrong.push((i, seen));
            }
        }
        println!(
            "{name}: {} of 600 boxes not shown in their colour",
            wrong.len()
        );
        assert!(
            wrong.is_empty(),
            "{name}: boxes missing or miscoloured: {wrong:?}"
        );

        // Batched boxes that move or go rebuild their batch: lift red box 4 by a
        // metre and remove red box 0.
        let pixel = |at: Vec3| {
            let ndc = projection.project_point3(view.transform_point3(at));
            (
                ((ndc.x * 0.5 + 0.5) * SIZE[0] as f32) as u32,
                ((0.5 - ndc.y * 0.5) * SIZE[1] as f32) as u32,
            )
        };
        let mut changed = crowd();
        let lifted = changed.draws[5].world * Mat4::from_translation(Vec3::Y);
        changed.draws[5].world = lifted;
        let gone = changed.draws.remove(1).world;
        let capture = presenter
            .capture_with(&changed, &request, || Ok(Content::default()))
            .unwrap();
        capture
            .image
            .save(out.join(format!("crowd-{name}-changed.png")))
            .unwrap();
        let (x, y) = pixel(lifted.transform_point3(Vec3::new(0., 0.15, 0.)));
        assert_eq!(
            hue(capture.image.get_pixel(x, y).0),
            Some(0),
            "{name}: lifted box"
        );
        let (x, y) = pixel(gone.transform_point3(Vec3::new(0., 0.15, 0.)));
        assert_ne!(
            hue(capture.image.get_pixel(x, y).0),
            Some(0),
            "{name}: removed box"
        );
    }
}
