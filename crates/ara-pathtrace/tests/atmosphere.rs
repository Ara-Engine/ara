use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;
use ara_pathtrace::scenes;
use ara_render::scene::{CameraSpec, Mesh, Vertex};
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, ToneMapper, Upscaler,
    atmosphere::Atmosphere,
};
use glam::Vec3;

#[test]
fn froxel_slice_end_sampling_matches_beer_lambert() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join(".sn/atmosphere-test-cache"))?;
    let mut scene = scenes::scene("furnace", 0);
    // An intentionally different extracted environment catches a fallback that
    // fails to use the selected physical sky, or fails to restore that environment.
    scene.environment.fill([10., 0., 0., 1.]);
    scene.materials[0].emissive_roughness = [1., 1., 1., 0.5];
    scene.meshes = vec![Mesh {
        entity: 1,
        name: "Unit-radiance plane at one kilometre".into(),
        material: 0,
        indices: vec![0, 1, 2, 0, 2, 3],
        vertices: [
            [-1000., -1000., -1000.],
            [1000., -1000., -1000.],
            [1000., 1000., -1000.],
            [-1000., 1000., -1000.],
        ]
        .map(|position| Vertex {
            position,
            normal: [0., 0., 1.],
            uv: [0., 0.],
            tangent: [1., 0., 0., 1.],
        })
        .to_vec(),
    }];
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
            tone_mapper: ToneMapper::PbrNeutral,
            bloom: 0.,
            fog_density: 0.,
            ..Default::default()
        },
    };
    let mut view = FidelityView::new(&mut gpu, &scene, settings, None)?;
    view.set_atmosphere(Some(Atmosphere {
        fog_extinction: 0.001,
        fog_height: 1e6,
        fog_albedo: 0.,
        cloud_coverage: 0.,
        ..Default::default()
    }))?;
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0., 2., 0.),
        target: Vec3::new(0., 2., -1.),
        vertical_fov: 0.8,
    };
    view.render(
        &mut gpu,
        &FrameInput {
            camera: &camera,
            tick: 0,
            delta_seconds: 1. / 60.,
            reset: true,
            lighting_changed: false,
            deterministic: true,
        },
    )?;
    let image = view.capture(&mut gpu)?;
    let actual = image.get_pixel(32, 18);
    let extinction = Vec3::new(5.802e-6, 13.558e-6, 33.1e-6) * (-2_f32 / 8000.).exp()
        + Vec3::splat(3.996e-6 / 0.9 * (-2_f32 / 1200.).exp() + 0.001 * (-2_f32 / 1e6).exp());
    let trans = Vec3::from_array((-extinction * 1000.).to_array().map(f32::exp));
    let mapped = ara_render_fidelity::colour::tone_map(trans / 1.2, ToneMapper::PbrNeutral);
    let expected = mapped.to_array().map(|linear| {
        let srgb = if linear <= 0.0031308 {
            12.92 * linear
        } else {
            1.055 * linear.powf(1. / 2.4) - 0.055
        };
        (srgb * 255.).round() as u8
    });
    for (channel, expected) in expected.into_iter().enumerate() {
        ensure!(
            actual[channel].abs_diff(expected) <= 2,
            "Atmosphere channel {channel}: {} vs Beer-Lambert {expected}",
            actual[channel]
        );
    }
    let bake = view.bake_snapshot(&mut gpu)?;
    view.destroy(&mut gpu)?;
    let mut raster = FidelityView::new(
        &mut gpu,
        &scene,
        Settings {
            tier: Tier::Raster,
            ..settings
        },
        Some(&bake),
    )?;
    raster.set_atmosphere(Some(Atmosphere {
        fog_extinction: 0.001,
        fog_height: 1e6,
        fog_albedo: 0.,
        cloud_coverage: 0.,
        ..Default::default()
    }))?;
    let frame = FrameInput {
        camera: &camera,
        tick: 0,
        delta_seconds: 1. / 60.,
        reset: true,
        lighting_changed: false,
        deterministic: true,
    };
    raster.render(&mut gpu, &frame)?;
    let image = raster.capture(&mut gpu)?;
    for (channel, expected) in expected.into_iter().enumerate() {
        ensure!(
            image.get_pixel(32, 18)[channel].abs_diff(expected) <= 2,
            "Baked Raster atmosphere channel {channel}"
        );
    }
    raster.set_atmosphere(None)?;
    raster.render(&mut gpu, &frame)?;
    let hdr = raster.capture_radiance(&mut gpu)?;
    ensure!(
        hdr[18 * 65 + 32][0] > 2.,
        "Disabling atmosphere must restore the red extracted environment"
    );
    raster.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
