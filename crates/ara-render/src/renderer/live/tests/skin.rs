use super::*;

#[test]
fn raster_skin_updates_reach_the_first_draw_without_a_separate_submission() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/raster-skin-submission");
    let mut scene = fixture::scene();
    let mut renderer = Renderer::new_native(&scene, Box::new(Textures), [96, 64], &[], &root)?;
    let request = crate::capture::CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 55f32.to_radians(),
        },
        size: [96, 64],
        mode: ViewMode::Albedo,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let before = renderer.capture(&request)?.image;
    before.save(root.join("before.png"))?;
    for vertex in &mut scene.geometry[0].vertices {
        vertex.position[0] += 0.4;
    }
    for bound in &mut scene.geometry[0].bounds {
        bound.x += 0.4;
    }
    scene.geometry[0].revision += 1;
    renderer.gpu.begin_deferred_completion_on_current_queue()?;
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            geometries: vec![0],
            ..Default::default()
        },
    )?;
    assert_eq!(
        renderer.gpu.deferred_submission_count(),
        0,
        "raster skin uploads must join their consuming draw, not submit a barrier-only pre-pass"
    );
    renderer.render_view(&request.camera, 1., ViewMode::Albedo)?;
    let mut completion = renderer.gpu.end_deferred_completion()?;
    renderer.gpu.wait_completion(&mut completion)?;
    let after = renderer.capture_completed(&request)?.image;
    after.save(root.join("after.png"))?;
    assert_ne!(
        before, after,
        "the first consuming frame must show the skin update"
    );
    // The synchronous player prepares outside a deferred render scope.
    for vertex in &mut scene.geometry[0].vertices {
        vertex.position[0] += 0.1;
    }
    for bound in &mut scene.geometry[0].bounds {
        bound.x += 0.1;
    }
    scene.geometry[0].revision += 1;
    let waits = renderer.gpu.host_wait_stats().calls;
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            geometries: vec![0],
            ..Default::default()
        },
    )?;
    assert_eq!(
        renderer.gpu.host_wait_stats().calls,
        waits,
        "synchronous raster preparation must not wait for a barrier-only submission"
    );
    renderer.render_view(&request.camera, 1., ViewMode::Albedo)?;
    renderer.gpu.assert_clean()?;
    Ok(())
}
