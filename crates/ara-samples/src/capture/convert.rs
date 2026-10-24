//! Sample-specific asset resolution; scene conversion belongs to the engine.
use super::{
    look::{Look, Sky},
    world::World,
};
use crate::lock::Lock;
use anyhow::{Context, Result};
use ara_render::scene::{Material, Scene, Texture};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::fidelity::{self, Content, EnvironmentMap, Foliage, TerrainLayer};
pub use fidelity::Stats;
use std::path::Path;

/// Resolves the sample look's temporary asset overrides, then uses the engine adapter.
pub fn scene(
    world: &mut World,
    frame: &RenderFrame,
    look: &Look,
    lock: &Lock,
    root: &Path,
) -> Result<(Scene, Stats)> {
    let mut content = Content::default();
    for d in &look.details {
        content.details.insert(
            d.asset.clone(),
            ara_render_fidelity::material::MaterialDetail {
                clearcoat: d.clearcoat,
                sheen: d.sheen,
                transmission: d.transmission,
                ..Default::default()
            },
        );
    }
    if let Some(ground) = &look.terrain {
        content
            .terrain
            .push(texture_set(lock, root, &ground.texture, ground.tint)?);
        if let Some(rock) = &ground.rock {
            content
                .terrain
                .push(texture_set(lock, root, rock, ground.rock_tint)?);
        }
    }
    if let Some(Sky::Hdri {
        asset,
        intensity,
        rotation,
    }) = &look.sky
    {
        let entry = lock
            .get(asset)
            .with_context(|| format!("no HDRI {asset}"))?;
        let file = entry.entry(&[".hdr"]).context("HDRI file")?;
        let (size, pixels) = hdri(&entry.dir(root).join(&file.path), *intensity, *rotation)?;
        content.environment = Some(EnvironmentMap { size, pixels });
    }
    let mut rules = Vec::new();
    if let Some((_, doc)) = world.workspace.docs().find(|(_, d)| d.kind == "foliage") {
        for (id, record) in doc.records() {
            let value = serde_json::Value::Object(record.clone());
            if id.as_str().starts_with("x_") {
                content.exclusions.push(serde_json::from_value(value)?);
            } else if id.as_str().starts_with("r_") {
                let rule: ara_foliage::Rule = serde_json::from_value(value)?;
                if rule.enabled {
                    rules.push(rule);
                }
            }
        }
    }
    for rule in rules {
        let asset = ara_runtime::assets::asset_id(&rule.asset)?;
        let model = world.model(asset)?;
        content.foliage.push(Foliage { rule, model });
    }
    fidelity::scene(frame, content)
}

/// Resolves a pinned CC0 texture set. This stays sample-specific until its look is
/// represented by authored terrain material assets.
fn texture_set(lock: &Lock, root: &Path, id: &str, tint: [f32; 3]) -> Result<TerrainLayer> {
    let set = lock
        .get(id)
        .with_context(|| format!("no texture set {id}"))?;
    let mut textures = [None, None, None, None];
    for (slot, key, srgb) in [
        (0, "_diff_", true),
        (1, "_nor_gl_", false),
        (2, "_arm_", false),
    ] {
        let f = set
            .files
            .iter()
            .find(|f| f.path.contains(key))
            .with_context(|| format!("{id} has no {key}"))?;
        let image = image::open(set.dir(root).join(&f.path))?.to_rgba8();
        textures[slot] = Some(Texture {
            size: [image.width(), image.height()],
            rgba: image.into_raw(),
            srgb,
        });
    }
    Ok(TerrainLayer {
        material: Material {
            base_color: [tint[0], tint[1], tint[2], 1.0],
            emissive_roughness: [0.0, 0.0, 0.0, 1.0],
            surface: [1.0, 1.0, 1.0, 0.5],
            ..Default::default()
        },
        textures,
        meters: set.meters.unwrap_or([2.0, 2.0]),
    })
}

/// An equirectangular HDRI scaled to cd/m², rotated about +Y and box-filtered to at
/// most 1024 × 512.
fn hdri(path: &Path, intensity: f32, rotation: f32) -> Result<([u32; 2], Vec<[f32; 4]>)> {
    let image = image::open(path)?.to_rgb32f();
    let (w, h) = (image.width(), image.height());
    let factor = (w / 1024).max(1);
    let (ow, oh) = (w / factor, h / factor);
    let shift = ((rotation / 360.0).rem_euclid(1.0) * ow as f32) as u32;
    let mut out = vec![[0.0f32; 4]; (ow * oh) as usize];
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = [0.0f32; 3];
            for dy in 0..factor {
                for dx in 0..factor {
                    let p = image.get_pixel(x * factor + dx, y * factor + dy);
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                }
            }
            let n = (factor * factor) as f32;
            let tx = (x + shift) % ow;
            out[(y * ow + tx) as usize] = [
                sum[0] / n * intensity,
                sum[1] / n * intensity,
                sum[2] / n * intensity,
                1.0,
            ];
        }
    }
    Ok(([ow, oh], out))
}
