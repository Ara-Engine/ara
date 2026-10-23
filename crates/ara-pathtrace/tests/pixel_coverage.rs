use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::{CameraSpec, Mesh, Vertex};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use glam::Vec3;

#[test]
fn shared_diffuse_specular_path_preserves_furnace_energy() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/shared-furnace-cache"))?;
    let mut scene = ara_pathtrace::scenes::scene("furnace", 0);
    scene.sun_lux = Vec3::ZERO;
    scene.lights.clear();
    scene.environment.fill([1.; 4]);
    scene.materials[0].base_color = [0.8, 0.8, 0.8, 1.];
    scene.materials[0].emissive_roughness = [0., 0., 0., 1.];
    scene.materials[0].surface[0] = 0.;
    scene.meshes = vec![Mesh {
        entity: 1,
        name: "Furnace receiver".into(),
        material: 0,
        indices: vec![0, 1, 2, 0, 2, 3],
        vertices: [[-3., -3., 0.], [3., -3., 0.], [3., 3., 0.], [-3., 3., 0.]]
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                uv: [0.; 2],
                tangent: [1., 0., 0., 1.],
            })
            .to_vec(),
    }];
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 5.),
        target: Vec3::ZERO,
        vertical_fov: 0.2,
    };
    let post = PostSettings {
        ev100: Some(0.),
        bloom: 0.,
        ..Default::default()
    };
    let mut reference = ara_pathtrace::Reference::new(&mut gpu, &scene, [65, 37], 4096, 2, post)?;
    reference.converge(&mut gpu, &camera)?;
    let ground = reference.radiance(&mut gpu)?;
    reference.destroy(&mut gpu)?;
    let settings = Settings {
        render_size: [65, 37],
        display_size: [65, 37],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 0.6,
        post,
    };
    let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
    view.set_reservoir_sampling(&mut gpu, true)?;
    for tick in 0..512 {
        view.render(
            &mut gpu,
            &FrameInput {
                camera: &camera,
                tick,
                delta_seconds: 1. / 60.,
                reset: false,
                lighting_changed: false,
                deterministic: false,
            },
        )?;
    }
    let data = view.capture_radiance(&mut gpu)?;
    let mean = |p: &[[f32; 4]]| {
        (17..20)
            .flat_map(|y| (31..34).map(move |x| y * 65 + x))
            .map(|i| p[i][0])
            .sum::<f32>()
            / 9.
    };
    let actual = mean(&data);
    let expected = mean(&ground);
    println!("Shared-path furnace: expected {expected}, measured {actual}");
    view.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    ensure!(
        (actual / expected - 1.).abs() < 0.03,
        "Shared lobe proposal lost energy: expected {expected}, got {actual}"
    );
    Ok(())
}

#[test]
fn mixed_rate_specular_preserves_subpixel_material_coverage() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/pixel-coverage-cache"))?;
    let mut scene = ara_pathtrace::scenes::scene("furnace", 0);
    scene.sun_lux = Vec3::ZERO;
    scene.lights.clear();
    scene.environment.fill([1.; 4]);
    scene.meshes.clear();
    let mut metal = scene.materials[0];
    metal.base_color = [0.8, 0.8, 0.8, 1.];
    metal.emissive_roughness = [0., 0., 0., 0.2];
    metal.surface[0] = 1.;
    let mut black = metal;
    black.base_color = [0., 0., 0., 1.];
    black.emissive_roughness[3] = 1.;
    black.surface[0] = 0.;
    scene.materials = vec![metal, black];
    for (material, x0, x1) in [(0, -3., 0.), (1, 0., 3.)] {
        scene.meshes.push(Mesh {
            entity: material as u64 + 1,
            name: "Coverage half-plane".into(),
            material,
            indices: vec![0, 1, 2, 0, 2, 3],
            vertices: [[x0, -3., 0.], [x1, -3., 0.], [x1, 3., 0.], [x0, 3., 0.]]
                .map(|position| Vertex {
                    position,
                    normal: [0., 0., 1.],
                    uv: [0.; 2],
                    tangent: [1., 0., 0., 1.],
                })
                .to_vec(),
        });
    }
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 5.),
        target: Vec3::ZERO,
        vertical_fov: 0.2,
    };
    let post = PostSettings {
        ev100: Some(0.),
        bloom: 0.,
        ..Default::default()
    };
    let mut reference = ara_pathtrace::Reference::new(&mut gpu, &scene, [65, 37], 4096, 2, post)?;
    reference.converge(&mut gpu, &camera)?;
    let expected = reference.radiance(&mut gpu)?[18 * 65 + 32][0];
    reference.destroy(&mut gpu)?;
    let settings = Settings {
        render_size: [65, 37],
        display_size: [65, 37],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 0.6,
        post,
    };
    let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
    view.set_reservoir_sampling(&mut gpu, true)?;
    for tick in 0..512 {
        view.render(
            &mut gpu,
            &FrameInput {
                camera: &camera,
                tick,
                delta_seconds: 1. / 60.,
                reset: false,
                lighting_changed: false,
                deterministic: false,
            },
        )?;
    }
    let actual = view.capture_radiance(&mut gpu)?[18 * 65 + 32][0];
    println!("Mixed glossy/rough subpixel coverage: reference {expected}, hybrid {actual}");
    view.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    ensure!(
        (actual / expected - 1.).abs() < 0.05,
        "Variable-rate lighting biased pixel-area coverage: reference {expected}, hybrid {actual}"
    );
    Ok(())
}
