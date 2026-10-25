use super::*;
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    scene::CameraSpec,
};

#[test]
fn model_bounds_style_never_marks_authored_shapes_with_the_same_colour() {
    let mut frame = RenderFrame::default();
    frame.draws = vec![draw(1), draw(2)];
    for draw in &mut frame.draws {
        draw.color = crate::PENDING_MODEL_COLOR;
    }
    let prepared = prepare(
        &frame,
        Content {
            unpublished_models: BTreeMap::from([("e_1".into(), "as_pending".into())]),
            ..Default::default()
        },
        config(),
    )
    .unwrap();
    let flags = |i: usize| {
        prepared.scene.materials[prepared.scene.instances[i].material]
            .pbr
            .flags[1]
    };
    assert_ne!(flags(0) & ara_render::scene::MODEL_BOUNDS, 0);
    assert_eq!(flags(1) & ara_render::scene::MODEL_BOUNDS, 0);
}

#[test]
fn pending_model_bounds_leave_the_view_open_and_failed_models_stay_solid() -> Result<()> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/pending-bounds")
        .join(std::process::id().to_string());
    let mut pending = draw(1);
    pending.world = glam::Mat4::IDENTITY;
    pending.geometry = Geometry::Shape {
        shape: MeshShape::Box,
        size: [3., 3., 1.],
    };
    pending.color = crate::PENDING_MODEL_COLOR;
    pending.emissive = [0.; 3];
    pending.emissive_factor = None;
    let mut background = draw(2);
    background.world = glam::Mat4::from_translation(glam::Vec3::new(0., 0., -1.));
    background.geometry = Geometry::Shape {
        shape: MeshShape::Box,
        size: [12., 12., 0.1],
    };
    background.color = [0., 0., 0., 1.];
    background.emissive = [0.12, 0.2, 0.28];
    background.emissive_factor = None;
    let mut frame = RenderFrame::default();
    frame.environment.sky.0 = [0., 0., 0., 1.];
    frame.environment.horizon.0 = [0., 0., 0., 1.];
    frame.environment.ground.0 = [0., 0., 0., 1.];
    frame.sun = Some(([0., 0., 1.], [2.; 3]));
    frame.draws = vec![pending, background];
    let content = Content {
        unpublished_models: BTreeMap::from([("e_1".into(), "as_pending".into())]),
        ..Default::default()
    };
    let mut prepared = prepare(&frame, content, config())?;
    prepared.enable_motion();
    let mut renderer = Renderer::new_native(
        &prepared.scene,
        Box::new(prepared.textures.fork_sources(config())?),
        [192, 128],
        &[],
        root.join("gpu"),
    )?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: glam30::Vec3::new(0., 0., 4.),
            target: glam30::Vec3::ZERO,
            vertical_fov: 1.,
        },
        size: [192, 128],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let bounds = prepared.scene.instances[0].material;
    // Keep a paired view of the old filled fallback for visual review.
    prepared.scene.materials[bounds].pbr.flags[1] &= !ara_render::scene::MODEL_BOUNDS;
    renderer.update_native(&prepared.scene)?;
    renderer
        .capture(&request)?
        .write(&root.join("filled-before.png"))?;
    prepared.scene.materials[bounds].pbr.flags[1] |= ara_render::scene::MODEL_BOUNDS;
    renderer.update_native(&prepared.scene)?;
    let pending = renderer.capture(&request)?;
    pending.write(&root.join("pending.png"))?;
    let pending_pixels = (0..128)
        .flat_map(|y| (0..192).map(move |x| (x, y)))
        .filter(|&(x, y)| pending.entity_at(x, y) == Some(1))
        .count();
    eprintln!(
        "pending coverage {pending_pixels}/24576; {}",
        root.display()
    );
    assert_eq!(
        pending.entity_at(96, 64),
        Some(2),
        "loading bounds must not cover their interior"
    );
    assert!(
        pending_pixels > 0 && pending_pixels < 512,
        "short corner marks only, got {pending_pixels}"
    );

    // Status changes use the existing material row, with no geometry replacement.
    frame.draws[0].color = [1., 0., 1., 1.];
    prepared.update_frame(&frame)?;
    renderer.update_native_changed(&prepared.scene, prepared.changes())?;
    let failed = renderer.capture(&request)?;
    failed.write(&root.join("failed.png"))?;
    assert_eq!(failed.entity_at(96, 64), Some(1));

    frame.draws[0].color = crate::PENDING_MODEL_COLOR;
    frame.draws[0].world = glam::Mat4::from_scale(glam::Vec3::splat(10.));
    prepared.update_frame(&frame)?;
    renderer.update_native_changed(&prepared.scene, prepared.changes())?;
    let oversized = renderer.capture(&request)?;
    oversized.write(&root.join("oversized.png"))?;
    assert_eq!(
        oversized.entity_at(96, 64),
        Some(2),
        "an unknown large source must not obscure the camera"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
