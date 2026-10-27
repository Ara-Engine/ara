//! Virtual shadow maps (look::vsm): pages the frame's pixels mark render once their
//! frame completes, shadow ground far past the sun maps, and redraw when a caster
//! moves off them.
use anyhow::{Result, ensure};
use ara_gpu::{Access, GpuDevice, Resource};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced::{
        Geometry, Instance, Lod, Material, Scene, TextureBinding, TextureStats, Textures, Vertex,
    },
    look::{Quality, Shadows, VirtualShadows},
    scene::CameraSpec,
};
use glam::{Mat4, Vec3};

struct NoTextures;
impl Textures for NoTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

/// A 400 m ground and a 20 x 100 m plate 10 m above it at `plate`, under a sun
/// 45 degrees up in -X: the plate's shadow covers x 0..20 m below it.
fn scene(plate: Vec3) -> Scene {
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
    let elevation = 45f32.to_radians();
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
                Mat4::from_translation(plate) * Mat4::from_scale(Vec3::new(20., 1., 100.)),
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

/// Luminance at the image centre: the ground at x = 15 m, under the plate's shadow.
fn centre(renderer: &mut Renderer, name: &str) -> Result<f32> {
    let capture = renderer.capture(&CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(60., 15., 0.),
            target: Vec3::new(15., 0., 0.),
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
            .join("../../target/ara-render/vsm")
            .join(std::process::id().to_string())
            .join(format!("{name}.png")),
    )?;
    let p = capture.image.get_pixel(32, 32).0;
    Ok(0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
}

/// Captures until the centre's luminance satisfies `done` (pages land a frame after
/// the marks that request them complete), at most four times.
fn settle(renderer: &mut Renderer, name: &str, done: impl Fn(f32) -> bool) -> Result<f32> {
    let mut value = 0.;
    for attempt in 0..4 {
        value = centre(renderer, &format!("{name}-{attempt}"))?;
        if done(value) {
            break;
        }
    }
    Ok(value)
}

#[test]
fn virtual_pages_shadow_past_the_sun_maps_and_follow_moving_casters() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/vsm")
        .join(std::process::id().to_string());
    let plate = Vec3::new(0., 10., 0.);
    let mut renderer =
        Renderer::new_native(&scene(plate), Box::new(NoTextures), [64, 64], &[], cache)?;
    // Sun maps reach 2 m: the plate's shadow 45 m off can only come from pages.
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
    ensure!(lit > 40., "the ground is sunlit without pages: {lit}");
    renderer.set_quality(Quality {
        virtual_shadows: Some(VirtualShadows {
            pages: 256,
            budget: 64,
            bias: 0.,
        }),
        ..near
    })?;
    let shadowed = settle(&mut renderer, "pages", |v| v < lit * 0.5)?;
    eprintln!("ground under the plate: {lit} -> {shadowed}");
    ensure!(
        shadowed < lit * 0.5,
        "the plate's pages shadow it: {shadowed} vs {lit}"
    );
    // The plate leaves: the pages it touched redraw without it.
    renderer.update_native(&scene(plate + Vec3::new(0., 0., 150.)))?;
    let relit = settle(&mut renderer, "moved", |v| v > lit * 0.8)?;
    ensure!(
        relit > lit * 0.8,
        "the moved plate's pages are redrawn: {relit} vs {lit}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
