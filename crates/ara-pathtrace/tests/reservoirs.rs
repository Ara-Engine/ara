use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::{CameraSpec, Light, Mesh, Vertex};
use ara_render_fidelity::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
use glam::Vec3;

#[test]
fn area_light_occlusion_matches_independent_quad_integral() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/area-energy-test-cache"))?;
    let mut scene = ara_pathtrace::scenes::scene("furnace", 0);
    scene.lights.clear();
    scene.sun_lux = Vec3::ZERO;
    scene.environment.fill([0., 0., 0., 1.]);
    scene.meshes.clear();
    scene.materials[0].base_color = [1.; 4];
    scene.materials[0].emissive_roughness = [0., 0., 0., 1.];
    let mut black = scene.materials[0];
    black.base_color = [0., 0., 0., 1.];
    black.surface[0] = 1.;
    scene.materials.push(black);
    let black_id = scene.materials.len() - 1;
    let mut emitter = black;
    emitter.emissive_roughness = [5., 5., 5., 1.];
    scene.materials.push(emitter);
    let emitter_id = scene.materials.len() - 1;
    for (entity, material, p) in [
        (
            1,
            0,
            [[-3., -3., 0.], [3., -3., 0.], [3., 3., 0.], [-3., 3., 0.]],
        ),
        (
            2,
            black_id,
            [[0., -1., 1.], [1., -1., 1.], [1., 1., 1.], [0., 1., 1.]],
        ),
        (
            3,
            emitter_id,
            [
                [-1., -0.5, 2.],
                [1., -0.5, 2.],
                [1., 0.5, 2.],
                [-1., 0.5, 2.],
            ],
        ),
    ] {
        scene.meshes.push(Mesh {
            entity,
            name: "analytic area lighting".into(),
            material,
            indices: vec![0, 1, 2, 0, 2, 3],
            vertices: p
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
        position: Vec3::new(-0.25, 0., 1.5),
        target: Vec3::new(-0.25, 0., 0.),
        vertical_fov: 0.2,
    };
    let mut reference = ara_pathtrace::Reference::new(
        &mut gpu,
        &scene,
        [65, 37],
        4096,
        1,
        PostSettings {
            ev100: Some(0.),
            ..Default::default()
        },
    )?;
    reference.converge(&mut gpu, &camera)?;
    let data = reference.radiance(&mut gpu)?;
    let actual = (17..20)
        .flat_map(|y| (31..34).map(move |x| y * 65 + x))
        .map(|i| f64::from(data[i][0]))
        .sum::<f64>()
        / 9.;
    // Occluder x>=0 at z=1 hides emitter x>0.25 from receiver (-0.25,0,0).
    // Integrate the unoccluded rectangle directly, independently of RIS/CDF code.
    let mut expected = 0.;
    let n = 256;
    for y in 0..n {
        for x in 0..n {
            let dx = -1. + (f64::from(x) + 0.5) * 1.25 / f64::from(n) + 0.25;
            let dy = -0.5 + (f64::from(y) + 0.5) / f64::from(n);
            let d2 = dx * dx + dy * dy + 4.;
            let nl = 2. / d2.sqrt();
            let hv = ((1. + nl) * 0.5).sqrt();
            let fresnel = 0.04 + 0.96 * (1. - hv).powi(5);
            let bsdf =
                ((1. - fresnel) * nl + fresnel * nl / (2. * (nl + 1.))) / std::f64::consts::PI;
            expected += 5. * bsdf * nl / d2 * 1.25 / f64::from(n * n);
        }
    }
    println!("Area occlusion: expected {expected}, measured {actual}");
    ensure!(
        (actual / expected - 1.).abs() < 0.025,
        "Area-light RIS is biased: expected {expected}, got {actual}"
    );
    reference.destroy(&mut gpu)?;
    let settings = Settings {
        render_size: [65, 37],
        display_size: [65, 37],
        tier: Tier::Rt,
        mode: Mode::Hybrid,
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 0.6,
        post: PostSettings {
            ev100: Some(0.),
            bloom: 0.,
            ..Default::default()
        },
    };
    let mut hybrid = FidelityView::new(&mut gpu, &scene, settings, None)?;
    hybrid.set_reservoir_sampling(&mut gpu, true)?;
    for tick in 0..256 {
        hybrid.render(
            &mut gpu,
            &FrameInput {
                camera: &camera,
                tick,
                delta_seconds: 1. / 60.,
                reset: false,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
    }
    let data = hybrid.capture_direct_radiance(&mut gpu)?;
    let actual = (17..20)
        .flat_map(|y| (31..34).map(move |x| y * 65 + x))
        .map(|i| f64::from(data[i][0]))
        .sum::<f64>()
        / 9.;
    ensure!(
        (actual / expected - 1.).abs() < 0.05,
        "Reused area-light energy expected {expected}, got {actual}"
    );
    hybrid.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn thousand_identical_lights_preserve_analytic_energy_through_reuse() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/reservoir-test-cache"))?;
    let mut scene = ara_pathtrace::scenes::scene("furnace", 0);
    scene.environment.fill([0., 0., 0., 1.]);
    scene.sun_lux = Vec3::ZERO;
    scene.materials[0].base_color = [1.; 4];
    scene.materials[0].emissive_roughness = [0., 0., 0., 1.];
    scene.meshes = vec![Mesh {
        entity: 1,
        name: "Lambert/GGX energy plane".into(),
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
    let light = Light {
        position_range: [0., 0., 2., 1e6],
        color_lumens: [1., 1., 1., 16. * std::f32::consts::PI / 1000.],
        direction_outer: [0., 0., -1., -1.],
        parameters: [0.; 4],
    };
    scene.lights = vec![light; 1000];
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 0., 5.),
        target: Vec3::ZERO,
        vertical_fov: 0.2,
    };
    for mode in [
        Mode::Reference {
            samples: 256,
            bounces: 1,
        },
        Mode::Hybrid,
    ] {
        let settings = Settings {
            render_size: [65, 37],
            display_size: [65, 37],
            tier: Tier::Rt,
            mode,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 64,
            reflection_cutoff: 0.6,
            post: PostSettings {
                ev100: Some(0.),
                bloom: 0.,
                ..Default::default()
            },
        };
        let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
        if mode == Mode::Hybrid {
            view.set_reservoir_sampling(&mut gpu, true)?;
        }
        for tick in 0..128 {
            view.render(
                &mut gpu,
                &FrameInput {
                    camera: &camera,
                    tick,
                    delta_seconds: 1. / 60.,
                    reset: false,
                    lighting_changed: false,
                    deterministic: true,
                },
            )?;
        }
        let actual = view.capture_radiance(&mut gpu)?[18 * 65 + 32][0];
        // Unit incident illuminance: diffuse (1-F0)/pi plus rough GGX F0/(4*pi).
        let expected = 0.97 / std::f32::consts::PI;
        ensure!(
            (actual / expected - 1.).abs() < 0.015,
            "{mode:?}: analytic energy {expected}, got {actual}"
        );
        view.destroy(&mut gpu)?;
    }
    gpu.assert_clean()?;
    Ok(())
}
