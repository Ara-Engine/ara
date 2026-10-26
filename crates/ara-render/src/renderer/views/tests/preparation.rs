use super::*;
use ara_gpu::vulkan::{BufferPreparation, InitialData, ResourcePreparation};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(send) = self.0.take() {
            let _ = send.send(());
        }
    }
}

#[test]
fn blocked_view_allocation_preserves_primary_and_first_frames_use_prepared_extents() -> Result<()> {
    let mut renderer = Renderer::new_native(
        &fixture::scene(),
        Box::new(Textures(Arc::new(AtomicUsize::new(0)))),
        [128, 80],
        &[],
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/prepared-view"),
    )?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 1.,
    };
    renderer.render(&camera, 1.)?;
    let mut primary = (
        renderer.targets.images.to_vec(),
        renderer.targets.history,
        renderer.frame_number,
    );
    let baseline = renderer.gpu.descriptor_usage();
    let (entered, receive) = mpsc::channel();
    let (send, wait) = mpsc::channel();
    let release = Release(Some(send));
    let ticket = renderer
        .gpu
        .prepare_resources(|| ResourcePreparation {
            buffers: vec![BufferPreparation {
                size: 16,
                usage: vk::BufferUsageFlags::STORAGE_BUFFER,
                host: true,
                data: InitialData::deferred(move || {
                    entered.send(()).expect("test waits for allocator");
                    wait.recv().expect("test releases allocator");
                    Ok(vec![0; 16])
                }),
            }],
            images: vec![],
        })?
        .context("admitted allocation gate")?;
    receive.recv_timeout(Duration::from_secs(5))?;
    let mut view = renderer.create_view()?;
    let request = ViewRequest {
        camera: &camera,
        size: [96, 60],
        render_size: [64, 40],
        quality: *renderer.quality(),
        exposure: 1.,
    };
    let waits = renderer.gpu.context().host_wait_stats().calls;
    for _ in 0..8 {
        assert!(!renderer.preallocate_view(&mut view, &request)?);
        assert!(view.state.is_none());
        assert_eq!(renderer.gpu.descriptor_usage(), baseline);
        assert_eq!(renderer.gpu.context().host_wait_stats().calls, waits);
        assert_eq!(
            (
                renderer.targets.images.to_vec(),
                renderer.targets.history,
                renderer.frame_number
            ),
            primary
        );
    }
    renderer.submit_scene_frame(|r| r.render(&camera, 1.))?;
    assert!(!renderer.preallocate_view(&mut view, &request)?);
    let graphics_deadline = Instant::now() + Duration::from_secs(10);
    while !renderer.poll_scene_frame()? {
        ensure!(
            Instant::now() < graphics_deadline,
            "primary frame did not finish"
        );
        std::thread::yield_now();
    }
    assert!(!renderer.submit_scene_frame(|_| panic!("yield a boundary to prepared Game targets"))?);
    primary = (
        renderer.targets.images.to_vec(),
        renderer.targets.history,
        renderer.frame_number,
    );
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(10);
    let prepared = loop {
        if let Some(resources) = renderer.gpu.poll_prepared_resources(ticket)? {
            break resources;
        }
        ensure!(Instant::now() < deadline, "allocator gate did not complete");
        std::thread::sleep(Duration::from_millis(1));
    };
    targets::retire(&mut renderer.gpu, prepared)?;
    while !renderer.preallocate_view(&mut view, &request)? {
        ensure!(
            Instant::now() < deadline,
            "camera allocation did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        view.state
            .as_ref()
            .context("prepared view")?
            .matches_extent([64, 40], [96, 60])
    );
    assert_eq!(
        (
            renderer.targets.images.to_vec(),
            renderer.targets.history,
            renderer.frame_number
        ),
        primary
    );
    for _ in 0..2 {
        assert!(renderer.preallocate_view(&mut view, &request)?);
        renderer.submit_secondary(&mut view, &request, |_, _| Ok(true))?;
        renderer.wait_scene_frame()?;
        assert_eq!(
            renderer
                .view_output(&view)?
                .context("completed camera")?
                .size,
            [96, 60]
        );
    }

    let changed = ViewRequest {
        size: [120, 80],
        render_size: [80, 60],
        ..request
    };
    assert!(!renderer.preallocate_view(&mut view, &changed)?);
    assert_eq!(
        renderer
            .view_output(&view)?
            .context("last-good output")?
            .size,
        [96, 60]
    );
    renderer.release_view(view)?;
    renderer.gpu.collect()?;
    assert_eq!(renderer.gpu.descriptor_usage(), baseline);
    renderer.gpu.assert_clean()?;
    Ok(())
}

#[test]
fn prepared_view_binding_failure_retires_the_whole_batch_and_can_retry() -> Result<()> {
    let mut renderer = Renderer::new_native(
        &fixture::scene(),
        Box::new(Textures(Arc::new(AtomicUsize::new(0)))),
        [64, 48],
        &[],
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-render/prepared-view-rejection"),
    )?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 1.,
    };
    let request = ViewRequest {
        camera: &camera,
        size: [48, 32],
        render_size: [32, 24],
        quality: *renderer.quality(),
        exposure: 1.,
    };
    let baseline = renderer.gpu.descriptor_usage();
    let mut pressure = Vec::new();
    loop {
        let buffer = renderer
            .gpu
            .create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        if renderer.gpu.storage_buffer(buffer).is_err() {
            renderer.gpu.retire(Resource::Buffer(buffer))?;
            break;
        }
        pressure.push(buffer);
    }
    let occupied = renderer.gpu.descriptor_usage();
    let mut view = renderer.create_view()?;
    for _ in 0..3 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match renderer.preallocate_view(&mut view, &request) {
                Ok(false) => {}
                Ok(true) => panic!("full storage heap must reject camera constants"),
                Err(_) => break,
            }
            ensure!(
                Instant::now() < deadline,
                "prepared camera rejection timed out"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(renderer.gpu.descriptor_usage(), occupied);
        assert!(view.state.is_none());
    }
    for buffer in pressure {
        renderer.gpu.retire(Resource::Buffer(buffer))?;
    }
    renderer.gpu.collect()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !renderer.preallocate_view(&mut view, &request)? {
        ensure!(
            Instant::now() < deadline,
            "retry after heap pressure timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    renderer.release_view(view)?;
    renderer.gpu.collect()?;
    assert_eq!(renderer.gpu.descriptor_usage(), baseline);
    renderer.gpu.assert_clean()?;
    Ok(())
}

/// Explicit driver-allocation timing; prebuild, then run through ara-dev gpu.
#[test]
#[ignore = "explicit secondary-target preparation measurement"]
fn secondary_target_preparation_measurement() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/prepared-view-measurement");
    std::fs::create_dir_all(&root)?;
    let mut renderer = Renderer::new_native(
        &fixture::scene(),
        Box::new(Textures(Arc::new(AtomicUsize::new(0)))),
        [128, 80],
        &[],
        root.join("cache"),
    )?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 1.,
    };
    renderer.render(&camera, 1.)?;
    let request = ViewRequest {
        camera: &camera,
        size: [2560, 1440],
        render_size: [1707, 960],
        quality: *renderer.quality(),
        exposure: 1.,
    };
    let mut samples = Vec::new();
    for prepared in [false, true, false, true] {
        let mut view = renderer.create_view()?;
        let mut poll_max_ms = 0f64;
        let allocation = Instant::now();
        if prepared {
            let deadline = allocation + Duration::from_secs(10);
            loop {
                let began = Instant::now();
                let ready = renderer.preallocate_view(&mut view, &request)?;
                poll_max_ms = poll_max_ms.max(began.elapsed().as_secs_f64() * 1e3);
                if ready {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "measurement allocation timed out"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let allocation_ms = allocation.elapsed().as_secs_f64() * 1e3;
        let began = Instant::now();
        renderer.submit_secondary(&mut view, &request, |_, _| Ok(true))?;
        let submit_ms = began.elapsed().as_secs_f64() * 1e3;
        renderer.wait_scene_frame()?;
        samples.push(serde_json::json!({"prepared":prepared,"poll_max_ms":poll_max_ms,"allocation_ms":allocation_ms,"submit_ms":submit_ms,"ui_total_ms":poll_max_ms+submit_ms}));
        renderer.release_view(view)?;
    }
    let json = serde_json::to_string_pretty(&samples)?;
    std::fs::write(root.join("measurements.json"), &json)?;
    eprintln!("{json}");
    renderer.gpu.assert_clean()?;
    Ok(())
}

#[test]
fn warmed_quality_survives_target_adoption_without_changing_primary_resources() -> Result<()> {
    let mut renderer = Renderer::new_native(
        &fixture::scene(),
        Box::new(Textures(Arc::new(AtomicUsize::new(0)))),
        [96, 64],
        &[],
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-render/warmed-view-quality"),
    )?;
    let quality = crate::look::Quality {
        horizon: Some(1024),
        local_shadows: Some(crate::look::LocalShadows {
            lights: 1,
            resolution: 128,
            redraws: 1,
            in_gi_and_fog: true,
        }),
        virtual_shadows: Some(crate::look::VirtualShadows {
            pages: 256,
            budget: 8,
            bias: 1.,
        }),
        ..Default::default()
    };
    renderer.set_quality(quality)?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 1.,
    };
    renderer.render(&camera, 1.)?;
    // The first VSM render may retire the initial native visible-list capacity.
    // Baseline only live primary resources before adding the warmed camera.
    renderer.gpu.collect()?;
    let primary = (
        renderer.targets.images.to_vec(),
        renderer.targets.history,
        renderer.look.frame,
        renderer.look.vsm.as_ref().context("primary VSM")?.atlas,
    );
    let baseline = renderer.gpu.descriptor_usage();
    renderer.prewarm_view_quality(quality)?;
    let usage = renderer.gpu.descriptor_usage();
    renderer.prewarm_view_quality(quality)?;
    assert_eq!(
        usage,
        renderer.gpu.descriptor_usage(),
        "one cached camera, never one per poll"
    );
    assert_eq!(
        (
            renderer.targets.images.to_vec(),
            renderer.targets.history,
            renderer.look.frame,
            renderer.look.vsm.as_ref().context("primary VSM")?.atlas
        ),
        primary
    );
    // The Scene may render for a long time while the warmed Game camera is
    // still unclaimed. Its undefined, unused maps must not poison bindless use.
    renderer.render(&camera, 1.)?;
    renderer.gpu.assert_clean()?;
    let primary = (
        renderer.targets.images.to_vec(),
        renderer.targets.history,
        renderer.look.frame,
        renderer.look.vsm.as_ref().context("primary VSM")?.atlas,
    );
    let mut view = renderer.create_view()?;
    assert!(renderer.views.warm.is_none());
    let warm = view.state.as_ref().context("warmed camera")?;
    let maps = warm
        .look
        .cascades
        .as_ref()
        .context("warmed cascades")?
        .images
        .to_vec();
    let atlas = warm.look.vsm.as_ref().context("warmed VSM")?.atlas;
    let request = ViewRequest {
        camera: &camera,
        size: [80, 48],
        render_size: [48, 32],
        quality,
        exposure: 1.,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !renderer.preallocate_view(&mut view, &request)? {
        ensure!(
            Instant::now() < deadline,
            "warmed target adoption timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let ready = view.state.as_ref().context("prepared camera")?;
    assert_eq!(ready.look.quality, quality);
    assert_eq!(
        ready
            .look
            .cascades
            .as_ref()
            .context("retained cascades")?
            .images,
        maps
    );
    assert_eq!(
        ready.look.vsm.as_ref().context("retained VSM")?.atlas,
        atlas
    );
    renderer.submit_secondary(&mut view, &request, |_, _| Ok(true))?;
    renderer.wait_scene_frame()?;
    assert_eq!(
        (
            renderer.targets.images.to_vec(),
            renderer.targets.history,
            renderer.look.frame,
            renderer.look.vsm.as_ref().context("primary VSM")?.atlas
        ),
        primary
    );
    renderer.release_view(view)?;
    renderer.gpu.collect()?;
    assert_eq!(renderer.gpu.descriptor_usage(), baseline);
    renderer.gpu.assert_clean()?;
    Ok(())
}
