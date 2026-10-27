use super::*;

#[test]
fn blocked_secondary_retains_images_and_defers_shared_writes_and_release() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/async-game");
    let mut scene = fixture::scene();
    let mut renderer = Renderer::new_native(&scene, Box::new(Textures), [128, 80], &[], root)?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?;
    renderer.wait_scene_frame()?;
    let primary = (
        renderer.output(),
        renderer.entity_channel(),
        renderer.targets.history,
        renderer.frame_number,
    );
    let descriptors = renderer.gpu().descriptor_usage();
    let mut view = renderer.create_view()?;
    let mut request = crate::ViewRequest {
        camera: &camera,
        size: [72, 44],
        render_size: [72, 44],
        quality: *renderer.quality(),
        exposure: 1.,
    };
    assert!(
        renderer
            .submit_secondary(&mut view, &request, |_, _| Ok(true))?
            .is_none()
    );
    renderer.wait_scene_frame()?;
    let shown = renderer
        .view_output(&view)?
        .expect("completed first Game frame");
    let mut gate = Gate::new(renderer.gpu(), Queue::Graphics)?;
    scene.instances[0].world = Mat4::from_translation(Vec3::new(0.4, 0., 0.));
    let waits = renderer.gpu().host_wait_stats().calls;
    let output = renderer
        .submit_secondary(&mut view, &request, |r, _| {
            r.update_native_changed(
                &scene,
                &instanced::Changes {
                    instances: vec![0],
                    ..Default::default()
                },
            )?;
            Ok(true)
        })?
        .expect("retained Game frame");
    assert_eq!(
        renderer.gpu().host_wait_stats().calls,
        waits,
        "Game submission must not host-wait"
    );
    assert_eq!(output.image, shown.image);
    assert!(!renderer.poll_scene_frame()?);
    assert_eq!(
        (
            renderer.output(),
            renderer.entity_channel(),
            renderer.targets.history,
            renderer.frame_number
        ),
        primary
    );
    if renderer.gpu().context().graphics != renderer.gpu().context().scene_graphics {
        assert_eq!(renderer.entity_at([64, 40])?, Some(7));
    }
    request.size = [96, 60];
    request.render_size = [64, 40];
    assert_eq!(
        renderer
            .submit_secondary(&mut view, &request, |_, _| panic!(
                "busy Game must coalesce"
            ))?
            .expect("shown Game")
            .image,
        shown.image
    );
    gate.open()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !renderer.poll_scene_frame()? {
        ensure!(
            std::time::Instant::now() < deadline,
            "released Game frame did not finish"
        );
        std::thread::yield_now();
    }
    assert!(!renderer.submit_scene_frame(|_| panic!("yield one idle boundary to Game"))?);
    assert!(
        renderer.submit_scene_frame(|r| r.render_view(&camera, 1., ViewMode::Lit))?,
        "an expired Game burst must not hold the Scene reservation forever"
    );
    renderer.wait_scene_frame()?;
    renderer.submit_secondary(&mut view, &request, |_, _| Ok(true))?;
    renderer.wait_scene_frame()?;
    assert_eq!(
        renderer.view_output(&view)?.expect("resized Game").size,
        [96, 60]
    );
    for _ in 0..8 {
        assert!(
            renderer
                .submit_secondary(&mut view, &request, |r, _| {
                    let mut commands = r.gpu_mut().begin(Queue::Graphics);
                    commands.push(Command::BeginPass("RejectedGame.Prepare".into()));
                    commands.push(Command::EndPass);
                    r.gpu_mut().submit(commands.finish(), &[])?;
                    anyhow::bail!("reject after shared uploads")
                })
                .is_err()
        );
        renderer.wait_scene_frame()?;
        assert!(renderer.view_output(&view)?.is_some());
    }
    let mut gate = Gate::new(renderer.gpu(), Queue::Graphics)?;
    renderer.submit_secondary(&mut view, &request, |_, _| Ok(true))?;
    renderer.release_view(view)?;
    assert!(
        !renderer.poll_scene_frame()?,
        "dropping a Game must retain its completion token"
    );
    gate.open()?;
    renderer.wait_scene_frame()?;
    assert_eq!(renderer.gpu().descriptor_usage(), descriptors);
    let mut pressure = Vec::new();
    loop {
        let buffer =
            renderer
                .gpu_mut()
                .create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        match renderer.gpu_mut().storage_buffer(buffer) {
            Ok(_) => pressure.push(buffer),
            Err(_) => {
                renderer.gpu_mut().retire(Resource::Buffer(buffer))?;
                break;
            }
        }
    }
    renderer.gpu_mut().collect()?;
    let occupied = renderer.gpu().descriptor_usage();
    let mut rejected = renderer.create_view()?;
    for _ in 0..8 {
        assert!(
            renderer
                .submit_secondary(&mut rejected, &request, |_, _| Ok(true))
                .is_err()
        );
        renderer.wait_scene_frame()?;
        assert_eq!(
            renderer.gpu().descriptor_usage(),
            occupied,
            "failed camera construction must roll back allocations"
        );
    }
    renderer.release_view(rejected)?;
    for buffer in pressure {
        renderer.gpu_mut().retire(Resource::Buffer(buffer))?;
    }
    renderer.gpu_mut().collect()?;
    assert_eq!(renderer.gpu().descriptor_usage(), descriptors);
    renderer.gpu().assert_clean()?;
    Ok(())
}
