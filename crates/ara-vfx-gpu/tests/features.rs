#![allow(clippy::unwrap_used)]
use ara_gpu::vulkan::VulkanDevice;
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    scene::{Material, Scene, Texture, sphere},
};
use ara_vfx_gpu::{GpuSystem, SceneColor, SceneDepth, View};
use glam::Vec3;
use std::path::Path;

#[test]
fn ribbon_mesh_surface_and_depth_collision_execute_on_gpu() {
    if std::env::var("ARA_VFX_GPU_TESTS").as_deref() != Ok("1") {
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let captures = root.join("../../target/ara-vfx-gpu/features");
    std::fs::create_dir_all(&captures).unwrap();
    let mut gpu = VulkanDevice::new(&[], root.join(".cache/features")).unwrap();
    for (effect, tick) in ara_vfx::samples::additional() {
        let mut system = GpuSystem::new(
            &mut gpu,
            &effect,
            &root.join("cooked").join(&effect.name),
            [960, 540],
        )
        .unwrap();
        let request = CaptureRequest {
            camera: View::default().camera,
            size: [960, 540],
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 1.,
            settle_frames: 2,
            tick: u64::from(tick),
            seed: u64::from(effect.seed),
            marks: Some(1),
        };
        let capture = system.capture(&mut gpu, &request).unwrap();
        let stats = system.stats(&gpu).unwrap();
        assert_eq!(stats.tick, tick);
        assert!(stats.particles_alive > 0);
        assert_eq!(capture.legend.len(), 1, "{} visible geometry", effect.name);
        let particles = system.read_particles(&mut gpu).unwrap();
        for p in particles.iter().flatten().filter(|p| p.age >= 0.) {
            assert!(p.position.is_finite() && p.velocity.is_finite() && p.color.is_finite());
        }
        if effect.name == "depth_sparks" {
            assert!(
                stats.emitters[0].events > 0,
                "depth contacts emit GPU events"
            );
        }
        capture
            .write(&captures.join(format!("{}.png", effect.name)))
            .unwrap();
        system.retire(&mut gpu).unwrap();
    }
    gpu.context().assert_clean().unwrap();
}

#[test]
fn composites_into_renderer_hdr_and_preserves_imported_states() {
    if std::env::var("ARA_VFX_GPU_TESTS").as_deref() != Ok("1") {
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let captures = root.join("../../target/ara-vfx-gpu/composite");
    std::fs::create_dir_all(&captures).unwrap();
    let scene = Scene {
        meshes: vec![sphere(1, "sphere".into(), Vec3::new(-1.2, 0.5, 0.), 0.5, 0)],
        materials: vec![Material::default()],
        textures: vec![
            Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            },
            Texture {
                size: [1, 1],
                rgba: vec![128, 128, 255, 255],
                srgb: false,
            },
        ],
        lights: vec![],
        sun_direction: Vec3::new(0.4, 1., 0.3).normalize(),
        sun_lux: Vec3::splat(2.),
        environment: vec![[0.03, 0.06, 0.1, 1.]],
        environment_size: [1, 1],
    };
    let mut renderer =
        Renderer::new(scene, [960, 540], &[], root.join(".cache/composite")).unwrap();
    let effect = ara_vfx::samples::campfire();
    let mut system = GpuSystem::new(
        renderer.gpu_mut(),
        &effect,
        &root.join("cooked/campfire"),
        [960, 540],
    )
    .unwrap();
    let mut view = View::default();
    for _ in 0..30 {
        renderer.render(&view.camera, view.exposure).unwrap();
        let (image, sampled_index, state) = renderer.feature_depth();
        view.scene_depth = Some(SceneDepth {
            image,
            sampled_index,
            state,
        });
        let (image, sampled_index, state) = renderer.feature_color();
        view.scene_color = Some(SceneColor {
            image,
            sampled_index,
            state,
        });
        system.step(renderer.gpu_mut(), &view).unwrap();
    }
    let pixels = system.read_output(renderer.gpu_mut()).unwrap();
    let image = image::RgbaImage::from_raw(960, 540, pixels).unwrap();
    image.save(captures.join("scene-composite.png")).unwrap();
    assert!(system.stats(renderer.gpu()).unwrap().particles_alive > 0);
    system.retire(renderer.gpu_mut()).unwrap();
    renderer.render(&view.camera, 1.).unwrap();
    renderer.gpu().context().assert_clean().unwrap();
}
