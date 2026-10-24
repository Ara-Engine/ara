#![allow(clippy::unwrap_used)]
//! Probe GI follows edits: after an entity moves and settles, the spot it left
//! renders as if it had never stood there, without a full rebake.
use ara_render::capture::{Capture, ViewMode};
use ara_runtime::{
    components::MeshShape,
    extract::{CameraView, Draw, Geometry, RenderFrame},
};
use ara_runtime_render::Presenter;
use std::path::Path;

fn draw(entity: u64, size: [f32; 3], at: [f32; 3]) -> Draw {
    Draw {
        entity,
        address: format!("e_{entity}"),
        label: format!("Box {entity}"),
        geometry: Geometry::Shape {
            shape: MeshShape::Box,
            size,
        },
        world: glam34::Mat4::from_translation(glam34::Vec3::from_array(at)),
        color: [0.7, 0.7, 0.7, 1.],
        roughness: 0.8,
        metallic: 0.,
        emissive: [0.; 3],
        emissive_factor: None,
        material: None,
        skin: None,
    }
}

/// A 24 m slab under the sky and a 2x3x2 m block at `x`, seen from above.
fn yard(x: f32) -> RenderFrame {
    let mut frame = RenderFrame::default();
    let forward = glam34::Vec3::new(0., -0.53, -0.85).normalize();
    frame.camera = Some(CameraView {
        position: [0., 10., 16.],
        forward: forward.to_array(),
        fov: 50.,
    });
    frame.draws = vec![
        draw(1, [24., 0.5, 24.], [0., -0.25, 0.]),
        draw(2, [2., 3., 2.], [x, 1.5, 0.]),
    ];
    frame
}

/// Per-pixel largest channel difference.
fn difference(a: &Capture, b: &Capture) -> Vec<u8> {
    a.image
        .as_raw()
        .chunks(4)
        .zip(b.image.as_raw().chunks(4))
        .map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0))
        .collect()
}

#[test]
fn a_moved_entity_leaves_no_probe_gi_behind() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-render/gi-edits");
    let _ = std::fs::remove_dir_all(&root);
    let size = [320, 180];
    let capture = |presenter: &mut Presenter, frame: &RenderFrame| {
        presenter
            .capture(frame, size, ViewMode::Lit, None, 0)
            .unwrap()
    };

    let mut edited = Presenter::new(size, Vec::new(), root.join("edited"));
    let mut frame = yard(-5.);
    let start = capture(&mut edited, &frame);
    // Move the block 10 m: it leaves the probe surfaces while it moves and bakes
    // back in at its new place once it has rested.
    frame.draws[1].world = glam34::Mat4::from_translation(glam34::Vec3::new(5., 1.5, 0.));
    for _ in 0..40 {
        edited.prepare(&frame).unwrap();
    }
    let after = capture(&mut edited, &frame);
    // The same scene acquired from scratch.
    let mut fresh = Presenter::new(size, Vec::new(), root.join("fresh"));
    let reference = capture(&mut fresh, &frame);
    for (name, image) in [
        ("start", &start),
        ("after", &after),
        ("reference", &reference),
    ] {
        image.write(&root.join(format!("{name}.png"))).unwrap();
    }

    // Where the move changed the image (both spots), the edited view matches.
    let changed: Vec<usize> = difference(&start, &reference)
        .iter()
        .enumerate()
        .filter(|(_, d)| **d > 12)
        .map(|(i, _)| i)
        .collect();
    assert!(changed.len() > 500, "{} pixels changed", changed.len());
    let error = difference(&after, &reference);
    let mean = changed.iter().map(|&i| f64::from(error[i])).sum::<f64>() / changed.len() as f64;
    let worst = error.iter().copied().max().unwrap_or(0);
    eprintln!("old/new spots: mean error {mean:.2}, worst pixel {worst}");
    assert!(mean < 1.5 && worst <= 12, "mean {mean:.2}, worst {worst}");
}
