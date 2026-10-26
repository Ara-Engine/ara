//! Heightfield horizon shadows (look::horizon): distant landforms shadow terrain
//! far past the sun maps.
use anyhow::{Result, ensure};
use ara_gpu::{Access, Resource, vulkan::VulkanDevice};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced::{
        Geometry, Instance, Lod, Material, Scene, TextureBinding, TextureStats, Textures, Vertex,
    },
    look::{Quality, Shadows},
    scene::CameraSpec,
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

/// A 400 m ground and a 20 x 100 m plate 30 m above it at the origin, under a sun
/// 10 degrees up in -X: the plate's shadow reaches about 180 m along +X.
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
    let elevation = 10f32.to_radians();
    Scene {
        geometry: vec![Geometry {
            key: [5; 32],
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
            instance(1, Mat4::from_scale(Vec3::splat(400.))),
            instance(
                2,
                Mat4::from_translation(Vec3::new(0., 30., 0.))
                    * Mat4::from_scale(Vec3::new(20., 1., 100.)),
            ),
        ],
        materials: vec![material],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::new(-elevation.cos(), elevation.sin(), 0.),
        sun_lux: Vec3::splat(10_000.),
        environment: vec![[0.01, 0.01, 0.01, 1.]],
        environment_size: [1, 1],
    }
}

/// Luminance at the image centre: the ground 60 m along +X, in the plate's shadow.
fn centre(renderer: &mut Renderer, name: &str) -> Result<f32> {
    let capture = renderer.capture(&CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(150., 15., 0.),
            target: Vec3::new(60., 0., 0.),
            vertical_fov: 0.5,
        },
        size: [64, 64],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.0006,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    })?;
    capture.write(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-render/horizon")
            .join(std::process::id().to_string())
            .join(format!("{name}.png")),
    )?;
    let p = capture.image.get_pixel(32, 32).0;
    Ok(0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
}

#[test]
fn distant_landforms_shadow_ground_past_the_sun_maps() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/horizon")
        .join(std::process::id().to_string());
    let mut renderer = Renderer::new_native(&scene(), Box::new(NoTextures), [64, 64], &[], cache)?;
    // Sun maps reach 2 m: the plate's shadow 60 m off can only come from the horizon.
    let near = Quality {
        shadows: Shadows {
            cascades: 1,
            resolution: 256,
            distance: 2.,
            far: None,
        },
        ..Quality::default()
    };
    renderer.set_quality(near)?;
    let lit = centre(&mut renderer, "maps-only")?;
    renderer.set_quality(Quality {
        horizon: Some(512),
        ..near
    })?;
    let shadowed = centre(&mut renderer, "horizon")?;
    eprintln!("ground 60 m past the plate: {lit} -> {shadowed}");
    ensure!(lit > 40., "the ground is sunlit without the horizon: {lit}");
    ensure!(
        shadowed < lit * 0.5,
        "the plate's horizon shadows it: {shadowed} vs {lit}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
