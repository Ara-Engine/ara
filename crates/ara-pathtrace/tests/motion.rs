use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;
use ara_pathtrace::scenes;
use ara_render::scene::{CameraSpec, Mesh, Vertex};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use glam::Vec3;

#[test]
fn deformation_motion_uses_previous_surface_depth_for_one_frame() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join(".sn/motion-test-cache"))?;
    let mut scene = scenes::scene("furnace", 0);
    scene.meshes = vec![Mesh {
        entity: 1,
        name: "Deforming plane".into(),
        material: 0,
        indices: vec![0, 1, 2, 0, 2, 3],
        vertices: [[-3., -3., 0.], [3., -3., 0.], [3., 3., 0.], [-3., 3., 0.]]
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                uv: [0., 0.],
                tangent: [1., 0., 0., 1.],
            })
            .to_vec(),
    }];
    let size = [65, 37];
    let settings = Settings {
        render_size: size,
        display_size: size,
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 0.6,
        post: PostSettings {
            ev100: Some(0.),
            ..Default::default()
        },
    };
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 5.),
        target: Vec3::ZERO,
        vertical_fov: 0.8,
    };
    let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
    let mut frame = FrameInput {
        camera: &camera,
        tick: 0,
        delta_seconds: 1. / 60.,
        reset: false,
        lighting_changed: false,
        deterministic: true,
    };
    view.render(&mut gpu, &frame)?;
    for vertex in &mut scene.meshes[0].vertices {
        vertex.position[2] += 0.5;
    }
    view.update_scene(&mut gpu, &scene)?;
    for (tick, expected_depth) in [(1, 5.), (2, 4.5)] {
        frame.tick = tick;
        view.render(&mut gpu, &frame)?;
        let center = view.capture_motion(&mut gpu)?[(18 * size[0] + 32) as usize];
        ensure!(
            center[0].abs() < 1e-5
                && center[1].abs() < 1e-5
                && (center[2] - expected_depth).abs() < 1e-4,
            "Frame {tick}: expected previous depth {expected_depth}, got {center:?}"
        );
    }
    view.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
