//! GPU pass timings of the native raster path for vertex colours and decals:
//! `cargo run -p ara-dev -- gpu -- cargo run --release -p ara-render --example native_timings`.
//! A 2560x1440 view over a 120 m floor and 4,096 boxes, measured plain, with every
//! box painted (the vertex-colour G-buffer PSO) and with 128, 512 and 1,024 decals
//! on the floor. Prints p50/p95 milliseconds of `GBuffer`, `Decals` and the frame.
use anyhow::Result;
use ara_gpu::{Access, GpuDevice, Resource};
use ara_render::{
    Renderer,
    instanced::{self, Changes, Decal, Geometry, Instance, Lod, Material, Scene, TextureStats, Textures, Vertex},
    scene::{CameraSpec, VERTEX_COLOR_TINT},
};
use glam::{Mat4, Vec3};
use std::collections::BTreeMap;

const SIZE: [u32; 2] = [2560, 1440];

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
    fn binding(&self) -> Option<instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

/// A unit box (24 vertices) or, `floor`, a 120 m quad.
fn geometry(key: u8, floor: bool) -> Geometry {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let faces: &[(Vec3, Vec3)] = if floor {
        &[(Vec3::Y, Vec3::X)]
    } else {
        &[(Vec3::X, Vec3::Z), (Vec3::NEG_X, Vec3::Z), (Vec3::Y, Vec3::X), (Vec3::NEG_Y, Vec3::X), (Vec3::Z, Vec3::X), (Vec3::NEG_Z, Vec3::X)]
    };
    let half = if floor { 60.0 } else { 0.5 };
    for &(n, t) in faces {
        let b = n.cross(t);
        let base = vertices.len() as u32;
        for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = if floor { (t * u + b * v) * half } else { n * 0.5 + (t * u + b * v) * 0.5 };
            vertices.push(Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                uv: [u * 0.5 + 0.5, v * 0.5 + 0.5],
                tangent: [t.x, t.y, t.z, 1.0],
                uv1: [0.0; 2],
                terrain_weights: 0,
                color: Vertex::WHITE,
            });
        }
        indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    let extent = if floor { Vec3::new(half, 0.0, half) } else { Vec3::splat(0.5) };
    Geometry {
        key: [key; 32],
        vertices,
        lods: vec![Lod { indices, error: 0.0 }],
        bounds: [-extent, extent],
        deforming: false,
        vertex_colors: false,
        revision: 0,
    }
}

fn scene(painted: bool, decals: usize) -> Scene {
    let material = |base: [f32; 4], flags: u32| Material {
        pbr: ara_render::scene::Material {
            base_color: base,
            textures: [u32::MAX; 4],
            flags: [0, flags, 0, 0],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut instances = vec![Instance {
        entity: 1,
        name: "floor".into(),
        geometry: 0,
        material: 0,
        world: Mat4::IDENTITY,
        previous_world: Mat4::IDENTITY,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    }];
    for i in 0..4096u64 {
        let (x, z) = ((i % 64) as f32 * 1.8 - 57.0, (i / 64) as f32 * 1.8 - 57.0);
        let world = Mat4::from_translation(Vec3::new(x, 0.5 + (i % 3) as f32 * 0.4, z));
        instances.push(Instance {
            entity: 10 + i,
            name: format!("box-{i}"),
            geometry: 1,
            material: 1,
            world,
            previous_world: world,
            casts_shadows: true,
            paint: painted.then(|| vec![instanced::pack_color([1.0, (i % 7) as f32 / 7.0, 0.3, 1.0]); 24].into()),
            receives_decals: true,
        });
    }
    let decals = (0..decals)
        .map(|i| {
            let (x, z) = ((i % 32) as f32 * 3.6 - 56.0, (i / 32) as f32 * 3.6 - 56.0);
            Decal {
                entity: 100_000 + i as u64,
                name: format!("decal-{i}"),
                world: Mat4::from_translation(Vec3::new(x, 0.0, z))
                    * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                    * Mat4::from_scale(Vec3::new(3.0, 3.0, 1.0)),
                material: 2,
                opacity: [0.8; 7],
                opacity_channel: 4,
                occlusion: [1.0, 4.0],
                specular: 0.5,
                fade_distance: [0.0, 0.0],
                fade_angle: [60.0, 80.0],
                sort_order: (i % 4) as i32,
                fade_screen_size: 0.0,
            }
        })
        .collect();
    Scene {
        geometry: vec![geometry(1, true), geometry(2, false)],
        instances,
        materials: vec![
            material([0.4, 0.4, 0.4, 1.0], 0),
            material([0.6, 0.6, 0.6, 1.0], VERTEX_COLOR_TINT),
            material([0.2, 0.05, 0.02, 1.0], 0),
        ],
        decals,
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::new(0.3, 1.0, 0.2).normalize(),
        sun_lux: Vec3::splat(80_000.0),
        environment: vec![[0.2, 0.25, 0.3, 1.0]],
        environment_size: [1, 1],
    }
}

fn main() -> Result<()> {
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0.0, 25.0, 45.0),
        target: Vec3::new(0.0, 0.0, -10.0),
        vertical_fov: 60f32.to_radians(),
    };
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/native-timings");
    let mut renderer = Renderer::new_native(&scene(false, 0), Box::new(NoTextures), SIZE, &[], cache)?;
    let mut report = BTreeMap::new();
    for (name, painted, decals) in [("plain", false, 0), ("painted", true, 0), ("decals128", false, 128), ("decals512", false, 512), ("decals1024", false, 1024)] {
        let scene = scene(painted, decals);
        renderer.replace_native_scene(&scene, Box::new(NoTextures))?;
        let mut samples = BTreeMap::<String, Vec<f64>>::new();
        for i in 0..160 {
            renderer.update_native_changed(&scene, &Changes::default())?;
            renderer.render(&camera, 1.0 / 80_000.0)?;
            if i >= 32 {
                let mut frame = 0.0;
                for timing in renderer.timings() {
                    frame += timing.milliseconds;
                    if matches!(timing.pass.as_str(), "GBuffer" | "Decals") {
                        samples.entry(timing.pass.clone()).or_default().push(timing.milliseconds);
                    }
                }
                samples.entry("frame".into()).or_default().push(frame);
            }
        }
        let mut row = BTreeMap::new();
        for (pass, mut times) in samples {
            times.sort_by(f64::total_cmp);
            row.insert(pass, serde_json::json!({"p50_ms": times[times.len() / 2], "p95_ms": times[times.len() * 95 / 100]}));
        }
        report.insert(name, row);
    }
    let json = serde_json::json!({"device": renderer.gpu().info().name, "size": SIZE, "scenarios": report});
    println!("{}", serde_json::to_string_pretty(&json)?);
    renderer.gpu().assert_clean()?;
    Ok(())
}
