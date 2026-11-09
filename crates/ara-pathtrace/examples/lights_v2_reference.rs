//! Path-traced reference and FLIP for the V2 many-light G1 bench (ara-render
//! `look::lights`): rebuilds the scene `scene.json` describes (unit boxes, materials,
//! the 112-byte light records with their authored ranges, the camera), renders the
//! undenoised direct-light reference (one bounce, no sun, black sky: local lights
//! only, windowed falloff as the raster renderer applies it), and scores every
//! `<name>.f32` radiance image in the same directory against it, and `v2-omit.f32`
//! against `exact.f32` (overload's region line).
//!
//! FLIP uses `ara_pathtrace::flip` at 67 pixels per degree on display images made
//! identically for every input: exposure 1 / (1.2 * 2^EV100), the ACES fit, sRGB.
//! Regions are 64x64-pixel blocks; the worst block's mean is the region FLIP.
//!
//! `cargo run -p ara-pathtrace --example lights_v2_reference -- <case dir> <samples>`
use anyhow::{Context, Result, ensure};
use ara_gpu::GpuDevice;
use ara_render::scene::{CameraSpec, Light, Material, Mesh, Scene, Texture, Vertex};
use ara_render_fidelity::PostSettings;
use glam::{Mat4, Vec3};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct Placed {
    world: [f32; 16],
    material: usize,
}
#[derive(Deserialize)]
struct Mat {
    albedo: [f32; 3],
    roughness: f32,
    metal: f32,
}
#[derive(Deserialize)]
struct Record {
    position_range: [f32; 4],
    color_lumens: [f32; 4],
    direction_outer: [f32; 4],
    parameters: [f32; 4],
}
#[derive(Deserialize)]
struct Description {
    size: [u32; 2],
    camera: CameraSpec,
    ev100: f32,
    materials: Vec<Mat>,
    boxes: Vec<Placed>,
    lights: Vec<Record>,
}

/// A world-space unit cube under `world`, outward counter-clockwise faces.
fn cube(entity: u64, world: Mat4, material: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for face in 0..6 {
        let (normal, u, v) = match face {
            0 => (Vec3::X, -Vec3::Z, Vec3::Y),
            1 => (-Vec3::X, Vec3::Z, Vec3::Y),
            2 => (Vec3::Y, Vec3::X, -Vec3::Z),
            3 => (-Vec3::Y, Vec3::X, Vec3::Z),
            4 => (Vec3::Z, Vec3::X, Vec3::Y),
            _ => (-Vec3::Z, -Vec3::X, Vec3::Y),
        };
        let start = vertices.len() as u32;
        let corners: Vec<Vec3> = [(0., 0.), (1., 0.), (0., 1.), (1., 1.)]
            .iter()
            .map(|&(x, y)| world.transform_point3(normal * 0.5 + u * (x - 0.5) + v * (y - 0.5)))
            .collect();
        let n = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .normalize();
        let t = (corners[1] - corners[0]).normalize();
        for (k, c) in corners.iter().enumerate() {
            vertices.push(Vertex {
                position: (*c).into(),
                normal: n.into(),
                uv: [(k % 2) as f32, (k / 2) as f32],
                tangent: [t.x, t.y, t.z, 1.],
            });
        }
        indices.extend([start, start + 1, start + 3, start, start + 3, start + 2]);
    }
    Mesh {
        entity,
        name: format!("box-{entity}"),
        vertices,
        indices,
        material,
    }
}

fn scene(d: &Description) -> Scene {
    Scene {
        meshes: d
            .boxes
            .iter()
            .enumerate()
            .map(|(i, b)| cube(i as u64 + 1, Mat4::from_cols_array(&b.world), b.material))
            .collect(),
        materials: d
            .materials
            .iter()
            .map(|m| Material {
                base_color: [m.albedo[0], m.albedo[1], m.albedo[2], 1.],
                emissive_roughness: [0., 0., 0., m.roughness],
                surface: [m.metal, 1., 0., 0.5],
                ..Material::default()
            })
            .collect(),
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
        lights: d
            .lights
            .iter()
            .map(|l| Light {
                position_range: l.position_range,
                color_lumens: l.color_lumens,
                direction_outer: l.direction_outer,
                parameters: l.parameters,
                ..Light::ZERO
            })
            .collect(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0., 0., 0., 1.]],
        environment_size: [1, 1],
    }
}

/// The display image every input shares (see the module docs).
fn display(rgba: &[f32], size: [u32; 2], ev100: f32) -> image::RgbaImage {
    let exposure = 1. / (1.2 * 2f32.powf(ev100));
    let tone = |x: f32| {
        let x = (x * exposure).max(0.);
        let v = ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0., 1.);
        let s = if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1. / 2.4) - 0.055
        };
        (s * 255. + 0.5) as u8
    };
    image::RgbaImage::from_fn(size[0], size[1], |x, y| {
        let i = ((y * size[0] + x) * 4) as usize;
        image::Rgba([tone(rgba[i]), tone(rgba[i + 1]), tone(rgba[i + 2]), 255])
    })
}

/// Mean FLIP and the worst 64x64 block's mean.
fn regions(errors: &[f32], size: [u32; 2]) -> (f64, f64, [u32; 2]) {
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mean = errors.iter().map(|&e| f64::from(e)).sum::<f64>() / errors.len() as f64;
    let (mut worst, mut at) = (0f64, [0, 0]);
    for by in (0..h).step_by(64) {
        for bx in (0..w).step_by(64) {
            let (mut sum, mut n) = (0f64, 0usize);
            for y in by..(by + 64).min(h) {
                for x in bx..(bx + 64).min(w) {
                    sum += f64::from(errors[y * w + x]);
                    n += 1;
                }
            }
            let m = sum / n as f64;
            if m > worst {
                worst = m;
                at = [bx as u32, by as u32];
            }
        }
    }
    (mean, worst, at)
}

fn read_f32(path: &Path, size: [u32; 2]) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    ensure!(
        bytes.len() == (size[0] * size[1] * 16) as usize,
        "{} is not {size:?} RGBA32F",
        path.display()
    );
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(
        args.next()
            .context("usage: lights_v2_reference <case dir> [samples]")?,
    );
    let samples: u32 = args.next().map_or(Ok(256), |s| s.parse())?;
    let d: Description = serde_json::from_slice(&std::fs::read(dir.join("scene.json"))?)?;
    let reference_path = dir.join("reference.f32");
    if !reference_path.exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut gpu = GpuDevice::new(&[], root.join("../../target/ara-pathtrace/lights-v2"))?;
        let post = PostSettings {
            ev100: Some(d.ev100),
            bloom: 0.,
            fog_density: 0.,
            ..PostSettings::default()
        };
        let mut reference =
            ara_pathtrace::Reference::new(&mut gpu, &scene(&d), d.size, samples, 1, post)?;
        let started = std::time::Instant::now();
        reference.converge(&mut gpu, &d.camera)?;
        let radiance = reference.radiance(&mut gpu)?;
        reference.destroy(&mut gpu)?;
        let flat: Vec<u8> = radiance
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        std::fs::write(&reference_path, flat)?;
        eprintln!(
            "reference: {samples} samples in {:.1} s",
            started.elapsed().as_secs_f64()
        );
    }
    let reference = read_f32(&reference_path, d.size)?;
    let reference_image = display(&reference, d.size, d.ev100);
    reference_image.save(dir.join("reference.png"))?;
    let prepared = ara_pathtrace::flip::PreparedImage::new(
        &reference_image,
        ara_pathtrace::flip::PIXELS_PER_DEGREE,
    )?;
    let mut results = serde_json::Map::new();
    let mut names: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".f32") && n != "reference.f32")
        .collect();
    names.sort();
    for name in &names {
        let image = display(&read_f32(&dir.join(name), d.size)?, d.size, d.ev100);
        let test = ara_pathtrace::flip::PreparedImage::new(
            &image,
            ara_pathtrace::flip::PIXELS_PER_DEGREE,
        )?;
        let flip = prepared.compare(&test)?;
        let (mean, worst, at) = regions(&flip.errors, d.size);
        let stem = name.trim_end_matches(".f32");
        let map = image::GrayImage::from_fn(d.size[0], d.size[1], |x, y| {
            image::Luma([(flip.errors[(y * d.size[0] + x) as usize].clamp(0., 1.) * 255.) as u8])
        });
        map.save(dir.join(format!("flip-{stem}.png")))?;
        results.insert(
            stem.into(),
            serde_json::json!({"mean": mean, "worst_region": worst, "worst_region_at": at}),
        );
        eprintln!("{stem}: FLIP mean {mean:.5}, worst 64px region {worst:.5} at {at:?}");
    }
    // Overload's line: omission against the exact finite-range control.
    if names.iter().any(|n| n == "v2-omit.f32") && names.iter().any(|n| n == "exact.f32") {
        let exact = display(&read_f32(&dir.join("exact.f32"), d.size)?, d.size, d.ev100);
        let omit = display(
            &read_f32(&dir.join("v2-omit.f32"), d.size)?,
            d.size,
            d.ev100,
        );
        let flip =
            ara_pathtrace::flip::flip(&exact, &omit, ara_pathtrace::flip::PIXELS_PER_DEGREE)?;
        let (mean, worst, at) = regions(&flip.errors, d.size);
        results.insert(
            "v2-omit-vs-exact".into(),
            serde_json::json!({"mean": mean, "worst_region": worst, "worst_region_at": at}),
        );
        eprintln!("v2-omit vs exact: FLIP mean {mean:.5}, worst region {worst:.5}");
    }
    std::fs::write(
        dir.join("flip.json"),
        serde_json::to_vec_pretty(&serde_json::Value::Object(results))?,
    )?;
    Ok(())
}
