//! The A/B control: the same G1 scene through today's renderer (`Renderer`, the
//! deferred lighting pass with `look::local`'s tiled lights and High's cached shadows
//! for the four brightest lights in view). Sun, sky, probe GI, AO and SSR are off, so
//! its HDR target holds local light only (plus nothing else: the environment is black).
//!
//! Also exports the scene for the path-traced reference (`scene_json`), read by
//! `ara-pathtrace`'s `lights_v2_reference` example.
use super::scene::{self, Layout};
use crate::Renderer;
use crate::look::{LocalShadows, Quality};
use crate::scene::{Material, Mesh, Scene, Texture, Vertex};
use anyhow::Result;
use ara_gpu::commands::Command;
use ara_gpu::{Access, Barrier, Device, Queue, Resource};
use glam::{Affine3A, Vec3};

/// Today's High local-light budget (ara-render-fidelity presets): four shadowed lights,
/// 512-texel faces, 12 redraws a frame.
pub(crate) const HIGH_LOCAL_SHADOWS: LocalShadows = LocalShadows {
    lights: 4,
    resolution: 512,
    redraws: 12,
    in_gi_and_fog: false,
};

/// The control's quality: High's lighting PSO and local shadows, nothing that adds
/// light besides the local lights.
pub(crate) fn quality(local_shadows: Option<LocalShadows>) -> Quality {
    Quality {
        gi_budget: 0,
        bloom: false,
        high_tier: true,
        local_shadows,
        emitter_lights: 0,
        ..Quality::default()
    }
}

/// World-space cube mesh of `world` with material slot `material`.
fn cube_mesh(entity: u64, world: Affine3A, material: usize) -> Mesh {
    let (positions, indices) = scene::cube();
    let mut vertices = Vec::with_capacity(positions.len());
    for quad in positions.as_chunks::<4>().0 {
        let corner = |k: usize| world.transform_point3(Vec3::from(quad[k]));
        let (a, b, c) = (corner(0), corner(1), corner(2));
        let normal = (b - a).cross(c - a).normalize();
        let tangent = (b - a).normalize();
        for (k, uv) in [[0., 0.], [1., 0.], [0., 1.], [1., 1.]]
            .into_iter()
            .enumerate()
        {
            vertices.push(Vertex {
                position: corner(k).into(),
                normal: normal.into(),
                uv,
                tangent: [tangent.x, tangent.y, tangent.z, 1.],
            });
        }
    }
    Mesh {
        entity,
        name: format!("bench-{entity}"),
        vertices,
        indices,
        material,
    }
}

/// The case's frame as a legacy scene (meshes in world space).
pub(crate) fn scene_at(layout: &Layout, frame: u32) -> Scene {
    let f = layout.frame(frame);
    let materials = scene::materials()
        .iter()
        .map(|m| Material {
            base_color: [m.albedo[0], m.albedo[1], m.albedo[2], 1.],
            emissive_roughness: [0., 0., 0., m.roughness],
            surface: [m.metal, 1., 0., 0.5],
            ..Material::default()
        })
        .collect();
    let mut meshes = vec![cube_mesh(1, layout.floor().world, scene::FLOOR as usize)];
    meshes.extend(
        f.boxes
            .iter()
            .map(|b| cube_mesh(b.id, b.world, b.material as usize)),
    );
    Scene {
        meshes,
        materials,
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
        lights: f.lights,
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0., 0., 0., 1.]],
        environment_size: [1, 1],
    }
}

/// The scene description `lights_v2_reference` rebuilds: size, camera, exposure,
/// materials, every box's world transform and material, and the light records.
pub(crate) fn scene_json(layout: &Layout, frame: u32, size: [u32; 2]) -> serde_json::Value {
    let f = layout.frame(frame);
    let boxes: Vec<serde_json::Value> = std::iter::once(layout.floor())
        .chain(f.boxes.iter().copied())
        .map(|b| serde_json::json!({"world": glam::Mat4::from(b.world).to_cols_array(), "material": b.material}))
        .collect();
    let materials: Vec<serde_json::Value> = scene::materials()
        .iter()
        .map(
            |m| serde_json::json!({"albedo": m.albedo, "roughness": m.roughness, "metal": m.metal}),
        )
        .collect();
    let lights: Vec<serde_json::Value> = f
        .lights
        .iter()
        .map(|l| {
            serde_json::json!({"position_range": l.position_range, "color_lumens": l.color_lumens,
                "direction_outer": l.direction_outer, "parameters": l.parameters})
        })
        .collect();
    serde_json::json!({"case": layout.case.id, "frame": frame, "size": size, "camera": layout.camera,
        "ev100": layout.ev100, "materials": materials, "boxes": boxes, "lights": lights})
}

/// One control run: the HDR radiance of the last frame (RGBA, f32 per channel) and
/// every frame's pass timings.
pub(crate) struct Control {
    pub radiance: Vec<f32>,
    pub frames: Vec<Vec<(String, f64)>>,
}

/// Renders `frames` frames (after `warm`) of `layout`'s frame zero through today's
/// renderer at `size` with `local_shadows`.
pub(crate) fn run(
    layout: &Layout,
    size: [u32; 2],
    warm: u32,
    frames: u32,
    local_shadows: Option<LocalShadows>,
) -> Result<Control> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut renderer = Renderer::new_raster(
        scene_at(layout, 0),
        size,
        &[],
        root.join("../../target/ara-render/lights-control"),
    )?;
    renderer.set_quality(quality(local_shadows))?;
    // Pixel centres, as the V2 bench samples them: no temporal jitter.
    renderer.set_jitter_override(Some([0., 0.]));
    let mut timings = Vec::new();
    for i in 0..warm + frames {
        renderer.render(&layout.camera, 1.)?;
        if i >= warm {
            timings.push(
                renderer
                    .timings()
                    .iter()
                    .map(|t| (t.pass.clone(), t.milliseconds))
                    .collect(),
            );
        }
    }
    // The HDR target holds radiance times the frame's exposure (one here).
    let (image, _, state) = renderer.feature_color();
    let gpu = renderer.gpu_mut();
    let bytes = u64::from(size[0]) * u64::from(size[1]) * 8;
    let readback = gpu.create_readback_buffer(bytes)?;
    let mut c = gpu.begin(Queue::Graphics);
    c.barriers([Barrier {
        resource: Resource::Image(image),
        before: state.access,
        after: Access::CopySource,
        handoff: None,
    }]);
    c.push(Command::ReadImage {
        source: image,
        destination: readback,
    });
    c.barriers([
        Barrier {
            resource: Resource::Buffer(readback),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        },
        Barrier {
            resource: Resource::Image(image),
            before: Access::CopySource,
            after: state.access,
            handoff: None,
        },
    ]);
    let point = gpu.submit(c.finish(), &[])?;
    gpu.resolve(point)?;
    let data = gpu.read(readback)?;
    gpu.retire(Resource::Buffer(readback))?;
    let halves: &[u16] = bytemuck::cast_slice(&data[..bytes as usize]);
    let radiance = halves
        .iter()
        .map(|&h| super::harness::half_to_f32(h))
        .collect();
    Ok(Control {
        radiance,
        frames: timings,
    })
}
