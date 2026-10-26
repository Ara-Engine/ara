use super::*;
use ara_gpu::{
    vk,
    vulkan::{BufferPreparation, InitialData, ResourcePreparation},
};
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

fn replacement() -> Result<RasterContent> {
    let mut frame = RenderFrame::default();
    let mut draw = box_draw(1, [2.; 3], [0.; 3], [0.; 3]);
    draw.color = [0.1, 0.5, 0.2, 1.];
    frame.draws.push(draw);
    RasterContent::new(frame, Content::default())
}

#[test]
fn blocked_content_allocation_keeps_rendering_last_good_and_publishes_after_cancel() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/background-allocation");
    let mut frame = RenderFrame::default();
    frame.draws.push(box_draw(1, [2.; 3], [0.; 3], [0.; 3]));
    let candidate = RasterContent::new(frame, Content::default())?;
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
    let frame = raster.publish_content(&mut renderer, candidate, plan)?;
    let camera = CameraSpec::LookAt {
        position: glam30::Vec3::new(0., 1., 4.),
        target: glam30::Vec3::ZERO,
        vertical_fov: 55f32.to_radians(),
    };
    let request = CaptureRequest {
        camera,
        size: [96, 64],
        mode: ViewMode::Albedo,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let before = raster.capture(&mut renderer, &frame, &request)?;
    let baseline = renderer.gpu().descriptor_usage();
    let (send, receive) = mpsc::channel();
    let release = Release(Some(send));
    let (entered, started) = mpsc::channel();
    let blocker = renderer
        .gpu_mut()
        .prepare_resources(|| ResourcePreparation {
            buffers: vec![BufferPreparation {
                size: 4,
                usage: vk::BufferUsageFlags::TRANSFER_SRC,
                host: true,
                data: InitialData::deferred(move || {
                    let _ = entered.send(());
                    let _ = receive.recv();
                    Ok(vec![0; 4])
                }),
            }],
            images: Vec::new(),
        })?
        .expect("worker admitted");
    started.recv_timeout(Duration::from_secs(5))?;
    let mut cancelled = replacement()?;
    for _ in 0..4 {
        assert!(!cancelled.preallocate(&mut renderer)?);
        renderer.submit_scene_frame(|renderer| {
            raster
                .render(renderer, &frame, &camera, ViewMode::Albedo)
                .map(|_| ())
        })?;
        renderer.wait_scene_frame()?;
    }
    cancelled.destroy(&mut renderer)?;
    assert_eq!(renderer.gpu().descriptor_usage(), baseline);
    assert_eq!(
        raster.capture(&mut renderer, &frame, &request)?.image,
        before.image
    );
    renderer.gpu_mut().cancel_prepared_resources(blocker)?;
    drop(release);
    let mut next = replacement()?;
    let started = Instant::now();
    while !next.preallocate(&mut renderer)? {
        ensure!(
            started.elapsed() < Duration::from_secs(10),
            "content allocation timeout"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let frame = raster.publish_content(&mut renderer, next, plan)?;
    assert!(
        renderer
            .native_geometry_stats()
            .expect("native")
            .uploaded_bytes
            > 0
    );
    let after = raster.capture(&mut renderer, &frame, &request)?;
    assert_ne!(before.image, after.image);
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
