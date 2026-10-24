//! Sample-only look inputs; project geometry/materials use the shipping native adapter.
use super::{
    look::{Look, Sky},
    world::World,
};
use crate::lock::Lock;
use anyhow::{Context, Result};
use ara_gpu::vulkan::VulkanDevice;
use ara_render_fidelity::{FidelityView, Settings};
use ara_runtime::extract::{Geometry, RenderFrame};
use ara_runtime_render::fidelity::{
    EnvironmentMap,
    native::{self, NativeConfig, Prepared},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Native content counts before camera-dependent GPU LOD selection.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Draw instances sharing local geometry.
    pub instances: usize,
    /// Unique geometry allocations.
    pub geometries: usize,
    /// Finest-LOD triangles summed over all instances.
    pub finest_instanced_triangles: usize,
    /// Runtime-scattered foliage placements.
    pub foliage_instances: usize,
    /// Compressed texture identities referenced by the native scene.
    pub textures: usize,
    /// Local lights.
    pub lights: usize,
    /// Materials with nonzero emission.
    pub emissive_materials: usize,
}

/// Shared native residency budget, identical to player presentation.
pub fn config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1 << 30,
        upload_bytes: 16 << 20,
    }
}

/// Resolves the same native terrain splats and prefab foliage as Play. The old
/// look terrain textures are superseded by authoritative Header.materials assets.
pub fn scene(
    world: &World,
    frame: &RenderFrame,
    look: &Look,
    lock: &Lock,
    root: &Path,
) -> Result<(Prepared, Stats)> {
    if let Some(ground) = &look.terrain {
        eprintln!(
            "legacy look terrain {} {:?}, rock {:?} {:?} is superseded by scene Header.materials",
            ground.texture, ground.tint, ground.rock, ground.rock_tint
        );
    }
    let mut content = world.content(frame)?;
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
    let mut prepared = native::prepare(frame, content, config())?;
    // Optional look-development layers remain snapshot inputs, never source edits.
    for detail in &look.details {
        let entities: BTreeSet<_> = frame
            .draws
            .iter()
            .filter_map(|draw| match &draw.geometry {
                Geometry::Model { model, .. } if model.asset == detail.asset => Some(draw.entity),
                _ => None,
            })
            .collect();
        let mut replacements = BTreeMap::new();
        for instance in &mut prepared.scene.instances {
            if entities.contains(&instance.entity) {
                let index = if let Some(&index) = replacements.get(&instance.material) {
                    index
                } else {
                    let mut material = prepared.scene.materials[instance.material];
                    ara_render_fidelity::material::MaterialDetail {
                        clearcoat: detail.clearcoat,
                        sheen: detail.sheen,
                        transmission: detail.transmission,
                        ..Default::default()
                    }
                    .apply(&mut material.pbr)?;
                    let index = prepared.scene.materials.len();
                    prepared.scene.materials.push(material);
                    replacements.insert(instance.material, index);
                    index
                };
                instance.material = index;
            }
        }
    }
    let triangles = prepared
        .scene
        .instances
        .iter()
        .map(|instance| {
            prepared.scene.geometry[instance.geometry].lods[0]
                .indices
                .len()
                / 3
        })
        .sum();
    let stats = Stats {
        instances: prepared.scene.instances.len(),
        geometries: prepared.scene.geometry.len(),
        finest_instanced_triangles: triangles,
        foliage_instances: frame
            .terrain
            .as_ref()
            .map_or(0, |terrain| terrain.data.foliage.len()),
        textures: prepared.scene.textures as usize,
        lights: prepared.scene.lights.len(),
        emissive_materials: prepared
            .scene
            .materials
            .iter()
            .filter(|m| {
                m.pbr.emissive_roughness[..3]
                    .iter()
                    .any(|value| *value > 0.)
            })
            .count(),
    };
    Ok((prepared, stats))
}

/// Creates one view and finishes its compressed residency before AA/reference
/// accumulation. Reference and preset views run serially on the shared GPU heap.
pub fn view(
    gpu: &mut VulkanDevice,
    prepared: &Prepared,
    settings: Settings,
) -> Result<FidelityView> {
    let mut view = FidelityView::new_native(
        gpu,
        &prepared.scene,
        prepared.textures.fork_sources(config())?,
        settings,
        None,
    )?;
    let settled = (|| -> Result<()> {
        for _ in 0..256 {
            let stats = view.refine_native_textures(gpu)?;
            if stats.pending_textures == 0 {
                return Ok(());
            }
            anyhow::ensure!(
                stats.uploaded_bytes > 0,
                "native capture residency made no progress"
            );
        }
        anyhow::bail!("native capture residency did not settle")
    })();
    if let Err(error) = settled {
        view.destroy(gpu)?;
        return Err(error);
    }
    Ok(view)
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
