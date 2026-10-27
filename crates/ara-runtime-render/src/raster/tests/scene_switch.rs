use super::*;

fn content(intensity: f32) -> Result<RasterContent> {
    let mut frame = RenderFrame::default();
    // No post-process volumes: the look's defaults meter exposure automatically.
    frame.environment.intensity = intensity;
    frame.draws.push(box_draw(1, [2.; 3], [0.; 3], [0.; 3]));
    RasterContent::new(frame, Content::default())
}

#[test]
fn scene_publication_restarts_exposure_without_a_capture_reset() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/scene-switch")
        .join(std::process::id().to_string());
    let candidate = content(50_000.)?;
    let mut renderer = Renderer::new_native(
        candidate.native.scene(),
        Box::new(candidate.prepared.textures.fork_sources(texture_config())?),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    let mut raster = RasterView::new(root.join("probes"));
    raster.set_probe_gi(false);
    let plan = RasterPlan {
        render_size: [96, 64],
        display_size: [96, 64],
        quality: *renderer.quality(),
    };
    let camera = CameraSpec::LookAt {
        position: glam30::Vec3::new(0., 1., 4.),
        target: glam30::Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    let frame = raster.publish_scene(&mut renderer, candidate, plan)?;
    for _ in 0..32 {
        raster.render(&mut renderer, &frame, &camera, ViewMode::Lit)?;
    }
    let request = CaptureRequest {
        camera,
        size: [96, 64],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.0015,
        settle_frames: 1,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let mut images = Vec::new();
    for _ in 0..2 {
        let frame = raster.publish_scene(&mut renderer, content(2.)?, plan)?;
        for _ in 0..4 {
            raster.render(&mut renderer, &frame, &camera, ViewMode::Lit)?;
        }
        // Read the live result: capture/reset_capture would hide inherited exposure.
        images.push(renderer.capture_completed(&request)?.image);
    }
    assert_eq!(
        images[0], images[1],
        "new scene cannot adapt from the previous scene"
    );
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
