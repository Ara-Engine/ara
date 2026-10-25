//! Cached local-light shadows (look::local) on the native path: an occluder
//! between a point light and the ground darkens the ground under it.
use anyhow::{Result, ensure};
use ara_gpu::{Access, Resource, vulkan::VulkanDevice};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced::{
        Geometry, Instance, Lod, Material, Scene, TextureBinding, TextureStats, Textures, Vertex,
    },
    look::{LocalShadows, Quality},
    scene::{CameraSpec, Light},
};
use glam::{Mat4, Vec3};

struct NoTextures;
impl Textures for NoTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut VulkanDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut VulkanDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut VulkanDevice) -> Result<()> {
        Ok(())
    }
}

/// A unit quad facing up, a 10 m ground and a 0.6 m occluder 1 m above it, and
/// a point light 2.5 m up.
fn scene() -> Scene {
    let quad = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(x, z)| Vertex {
        position: [x, 0., z],
        normal: [0., 1., 0.],
        tangent: [1., 0., 0., 1.],
        uv: [0.; 2],
        uv1: [0.; 2],
        terrain_weights: 0,
        reserved: 0,
    });
    let mut material = Material::default();
    material.pbr.textures = [u32::MAX; 4];
    let instance = |entity: u64, world: Mat4| Instance {
        entity,
        name: format!("quad-{entity}"),
        geometry: 0,
        material: 0,
        world,
        previous_world: world,
    };
    Scene {
        geometry: vec![Geometry {
            key: [9; 32],
            vertices: quad.to_vec(),
            lods: vec![Lod {
                indices: vec![0, 2, 1, 0, 3, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.5, 0., -0.5), Vec3::new(0.5, 0., 0.5)],
            deforming: false,
            revision: 0,
        }],
        instances: vec![
            instance(1, Mat4::from_scale(Vec3::splat(10.))),
            instance(
                2,
                Mat4::from_translation(Vec3::Y) * Mat4::from_scale(Vec3::splat(0.6)),
            ),
        ],
        materials: vec![material],
        textures: 0,
        lights: vec![Light {
            position_range: [0., 2.5, 0., 20.],
            color_lumens: [1., 1., 1., 2000.],
            direction_outer: [0., -1., 0., 0.],
            parameters: [0., 1., 0., 0.],
        }],
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0.01, 0.01, 0.01, 1.]],
        environment_size: [1, 1],
    }
}

/// Luminance at the image centre (the ground under the occluder).
fn centre(renderer: &mut Renderer) -> Result<f32> {
    let capture = renderer.capture(&CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(4., 3., 4.),
            target: Vec3::ZERO,
            vertical_fov: 0.6,
        },
        size: [64, 64],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.05,
        settle_frames: 4,
        tick: 0,
        seed: 0,
        marks: None,
    })?;
    let name = if renderer.quality().local_shadows.is_some() {
        "shadowed"
    } else {
        "unshadowed"
    };
    capture.write(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-render/local-shadows")
            .join(std::process::id().to_string())
            .join(format!("{name}.png")),
    )?;
    let p = capture.image.get_pixel(32, 32).0;
    Ok(0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
}

#[test]
fn an_occluder_shadows_the_ground_under_a_local_light() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/local-shadows")
        .join(std::process::id().to_string());
    let mut renderer = Renderer::new_native(&scene(), Box::new(NoTextures), [64, 64], &[], cache)?;
    let unshadowed = Quality {
        local_shadows: None,
        ..Quality::default()
    };
    renderer.set_quality(unshadowed)?;
    let lit = centre(&mut renderer)?;
    renderer.set_quality(Quality {
        local_shadows: Some(LocalShadows {
            lights: 1,
            resolution: 256,
            redraws: 6,
        }),
        ..unshadowed
    })?;
    let shadowed = centre(&mut renderer)?;
    eprintln!("local-shadow centre luminance: {lit} -> {shadowed}");
    ensure!(lit > 40., "the ground is lit without shadows: {lit}");
    ensure!(
        shadowed < lit * 0.5,
        "the occluder shadows it: {shadowed} vs {lit}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
