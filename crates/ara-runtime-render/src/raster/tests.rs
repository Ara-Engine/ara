use super::*;
use ara_runtime::{
    components::MeshShape,
    extract::{Draw, Geometry},
};

fn box_draw(id: u64, size: [f32; 3], position: [f32; 3], emission: [f32; 3]) -> Draw {
    Draw {
        entity: id,
        address: format!("e_{id}"),
        label: format!("Box {id}"),
        geometry: Geometry::Shape {
            shape: MeshShape::Box,
            size,
        },
        world: glam34::Mat4::from_translation(glam34::Vec3::from_array(position)),
        color: [0.7, 0.7, 0.7, 1.],
        roughness: 0.8,
        metallic: 0.,
        emissive: emission,
        emissive_factor: None,
        material: None,
        skin: None,
    }
}

#[test]
fn first_capture_uploads_finished_emitter_search_without_another_prepare() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/emitter-capture")
        .join(std::process::id().to_string());
    let mut frame = RenderFrame::default();
    frame.environment.sky.0 = [0., 0., 0., 1.];
    frame.environment.horizon.0 = [0., 0., 0., 1.];
    frame.environment.ground.0 = [0., 0., 0., 1.];
    frame.environment.exposure = 1.;
    frame.draws = vec![
        box_draw(1, [8., 0.5, 8.], [0., -0.25, 0.], [0.; 3]),
        box_draw(2, [1.; 3], [0., 2., 0.], [100., 80., 60.]),
    ];
    let config = texture_config();
    let mut prepared = native::prepare(&frame, Content::default(), config)?;
    prepared.enable_motion();
    let job =
        crate::emitters::Job::new(&prepared.scene, &crate::gi::shading(&prepared), |_, _| None);
    ensure!(!job.is_empty(), "fixture needs a real emitter search");
    let mut renderer = Renderer::new_native(
        &prepared.scene,
        Box::new(prepared.textures.fork_sources(config)?),
        [160, 90],
        &[],
        root.join("gpu"),
    )?;
    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
    renderer.set_look(look.raster())?;
    let mut raster = RasterView::new(root.join("probes"));
    raster.set_probe_gi(false);
    raster.look = Some(look);
    raster.prepared = Some(prepared);
    // Keep the result out of prepare deterministically: capture itself must
    // adopt it and upload the lights, rather than relying on worker timing.
    let (release, wait) = std::sync::mpsc::channel();
    raster.emitter_job = Some(std::thread::spawn(move || {
        wait.recv().expect("fixture releases the emitter search");
        job.run()
    }));
    release.send(())?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: glam30::Vec3::new(0., 3., 7.),
            target: glam30::Vec3::new(0., 1., 0.),
            vertical_fov: 50f32.to_radians(),
        },
        size: [160, 90],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 4,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let first = raster.capture(&mut renderer, &frame, &request)?;
    assert!(!raster.emitter_lights.is_empty());
    let plan = ara_render_fidelity::presets::RasterPlan {
        render_size: [160, 90],
        display_size: [160, 90],
        quality: *renderer.quality(),
    };
    raster.prepare(&mut renderer, &frame, plan, look, || Ok(Content::default()))?;
    let after_prepare = raster.capture(&mut renderer, &frame, &request)?;
    first.write(&root.join("first.png"))?;
    after_prepare.write(&root.join("after-prepare.png"))?;
    assert_eq!(first.image, after_prepare.image);
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
