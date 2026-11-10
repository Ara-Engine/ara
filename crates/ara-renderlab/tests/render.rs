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
