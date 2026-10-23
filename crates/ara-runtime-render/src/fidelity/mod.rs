//! Engine-owned conversion of a runtime frame into a fidelity scene snapshot.
//! Asset resolution is completed by the caller; conversion has no project or sample paths.
use anyhow::Result;
use ara_render::scene::{self, Material, Mesh, Scene, Texture, Vertex};
use ara_runtime::{
    components::LightKind,
    extract::{Geometry, RenderFrame},
    model::{Model, Primitive},
};
use glam::Vec3;
use std::collections::BTreeMap;

mod atlas;
mod content;
#[cfg(test)]
mod foliage_tests;
mod packing;
mod terrain;
#[cfg(test)]
mod tests;
/// Particle composition over fidelity HDR radiance and depth.
pub mod vfx;
pub use content::{Content, EnvironmentMap, Foliage, TerrainLayer};

/// Scene textures the fidelity view accepts.
const TEXTURE_LIMIT: usize = 96;
/// LOD tolerance for props (metres of geometric error).
const PROP_ERROR: f32 = 0.002;
/// LOD tolerance for scattered foliage.
const FOLIAGE_ERROR: f32 = 0.01;

/// What went into the snapshot, for the timings report.
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Drawn objects before merging (draws, terrain tiles, foliage instances).
    pub objects: usize,
    /// Meshes after merging by material.
    pub meshes: usize,
    /// Triangles over every mesh (after LOD selection).
    pub triangles: usize,
    /// Triangles the finest LODs would have had.
    pub finest_triangles: usize,
    /// Foliage instances (all rules).
    pub foliage_instances: usize,
    /// Distinct textures referenced by the content.
    pub textures_wanted: usize,
    /// Textures kept within the fidelity budget.
    pub textures: usize,
    /// Materials whose maps were packed into atlas pages.
    pub atlased_materials: usize,
    /// Texture texels uploaded (finest levels).
    pub texels: u64,
    /// Estimated upload size of vertices, indices and textures (with mips), in MiB.
    pub upload_mib: f64,
    /// Local lights.
    pub lights: usize,
    /// Materials using an emissive term.
    pub emissive_materials: usize,
    /// Triangles by object name (top entries), before merging.
    pub breakdown: Vec<(String, usize)>,
}

/// Builds a scene from the frame and resolved presentation assets. This is an upload
/// boundary operation, not a per-frame extraction path.
pub fn scene(frame: &RenderFrame, mut content: Content) -> Result<(Scene, Stats)> {
    let mut out = Builder::default();
    for draw in &frame.draws {
        let palette = draw.skin.as_ref().map(|r| &frame.palettes[r.start..r.end]);
        let default_emissive = if matches!(&draw.geometry, Geometry::Model { .. }) {
            let factor = draw.emissive_factor.unwrap_or([0.0; 3]);
            std::array::from_fn(|i| factor[i] * draw.emissive[i])
        } else {
            draw.emissive
        };
        let mut material = Material {
            base_color: draw.color,
            emissive_roughness: [
                default_emissive[0],
                default_emissive[1],
                default_emissive[2],
                draw.roughness,
            ],
            surface: [draw.metallic, 1.0, 1.0, 0.5],
            textures: [0, 1, 0, 0],
            flags: [u32::from(draw.color[3] < 1.0) * 2, 0, 0, 0],
        };
        let mut vertices = Vec::new();
        let indices = match &draw.geometry {
            Geometry::Shape { shape, size } => {
                let source = crate::geometry::shape(*shape, *size);
                crate::geometry::transform(&source.vertices, draw.world, palette, &mut vertices);
                source.indices
            }
            Geometry::Model { model, primitive } => {
                let p = &model.primitives[*primitive];
                crate::geometry::transform(&p.vertices, draw.world, palette, &mut vertices);
                if let Some(m) = p.material.and_then(|m| model.materials.get(m)) {
                    out.model_material(
                        &mut material,
                        model,
                        m,
                        draw.color,
                        draw.emissive,
                        draw.emissive_factor,
                    )?;
                    if let Some(d) = content.details.get(model.asset.as_str()) {
                        d.apply(&mut material)?;
                    }
                }
                let scale = draw.world.to_scale_rotation_translation().0.max_element();
                out.stats.finest_triangles += p.indices.len() / 3;
                lod(p, extent(model) * scale, PROP_ERROR).to_vec()
            }
        };
        if let Geometry::Shape { .. } = draw.geometry {
            out.stats.finest_triangles += indices.len() / 3;
        }
        out.add(&draw.label, vertices, indices, material);
    }
    if let Some(view) = &frame.terrain {
        if !content.terrain.is_empty() {
            out.terrain(&view.data, std::mem::take(&mut content.terrain))?;
        }
        out.foliage(&content.foliage, &content.exclusions, &view.data)?;
    }
    let names = std::mem::take(&mut out.names);
    let mut stats = std::mem::take(&mut out.stats);
    let mut scene = out.finish(TEXTURE_LIMIT, &mut stats)?;
    scene.lights = lights(frame);
    let (direction, lux) = frame.sun.unwrap_or(([0.3, 1.0, 0.2], [0.0; 3]));
    scene.sun_direction = glam30::Vec3::from_array(direction).normalize();
    scene.sun_lux = glam30::Vec3::from_array(lux);
    if let Some(environment) = content.environment {
        scene.environment_size = environment.size;
        scene.environment = environment.pixels;
    } else {
        let (size, pixels) = crate::sky(&frame.environment);
        scene.environment_size = size;
        scene.environment = pixels;
    }
    stats.lights = scene.lights.len();
    stats.meshes = scene.meshes.len();
    stats.textures = scene.textures.len();
    stats.triangles = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
    stats.emissive_materials = scene
        .materials
        .iter()
        .filter(|m| m.emissive_roughness[..3].iter().any(|v| *v > 0.0))
        .count();
    stats.texels = scene
        .textures
        .iter()
        .map(|t| u64::from(t.size[0]) * u64::from(t.size[1]))
        .sum();
    let vertex_bytes: usize = scene
        .meshes
        .iter()
        .map(|m| m.vertices.len() * size_of::<Vertex>() + m.indices.len() * 4)
        .sum();
    stats.upload_mib = (vertex_bytes as f64 + stats.texels as f64 * 8.0) / (1024.0 * 1024.0);
    let mut breakdown: Vec<(String, usize)> = names.into_iter().collect();
    breakdown.sort_by_key(|b| std::cmp::Reverse(b.1));
    breakdown.truncate(24);
    stats.breakdown = breakdown;
    Ok((scene, stats))
}

/// Largest model-space dimension.
fn extent(model: &Model) -> f32 {
    let [min, max] = model.bounds;
    (Vec3::from_array(max) - Vec3::from_array(min)).max_element()
}

/// The coarsest cooked LOD whose geometric error stays within `tolerance` metres.
fn lod(p: &Primitive, extent: f32, tolerance: f32) -> &[u32] {
    p.lods
        .iter()
        .rev()
        .find(|l| l.error * extent <= tolerance && !l.indices.is_empty())
        .map_or(&p.indices[..], |l| &l.indices[..])
}

/// A material's bits, for deduplication.
fn key(m: &Material) -> Vec<u32> {
    m.base_color
        .iter()
        .chain(&m.emissive_roughness)
        .chain(&m.surface)
        .map(|v| v.to_bits())
        .chain(m.textures)
        .chain(m.flags)
        .collect()
}

/// Geometry merged under one material.
#[derive(Default)]
struct Group {
    name: String,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

struct Builder {
    textures: Vec<Texture>,
    /// (model digest, texture index) → texture slot.
    slots: BTreeMap<(String, usize), u32>,
    materials: Vec<Material>,
    by_key: BTreeMap<Vec<u32>, usize>,
    groups: BTreeMap<usize, Group>,
    names: BTreeMap<String, usize>,
    stats: Stats,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            textures: vec![solid([255; 4], false), solid([128, 128, 255, 255], false)],
            slots: BTreeMap::new(),
            materials: Vec::new(),
            by_key: BTreeMap::new(),
            groups: BTreeMap::new(),
            names: BTreeMap::new(),
            stats: Stats::default(),
        }
    }
}

impl Builder {
    fn add(
        &mut self,
        name: &str,
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        mut material: Material,
    ) {
        if vertices.is_empty() || indices.is_empty() {
            return;
        }
        self.stats.objects += 1;
        *self
            .names
            .entry(name.split(" (").next().unwrap_or(name).to_owned())
            .or_default() += indices.len() / 3;
        for c in &mut material.base_color[..3] {
            *c = c.clamp(0.0, 1.0);
        }
        let k = key(&material);
        let index = *self.by_key.entry(k).or_insert_with(|| {
            self.materials.push(material);
            self.materials.len() - 1
        });
        let group = self.groups.entry(index).or_default();
        if group.name.is_empty() {
            group.name = name.to_owned();
        }
        let offset = group.vertices.len() as u32;
        group.vertices.extend(vertices);
        group
            .indices
            .extend(indices.into_iter().map(|i| i + offset));
    }

    fn texture(&mut self, model: &Model, index: usize) -> Result<u32> {
        let key = (model.digest.to_string(), index);
        if let Some(&slot) = self.slots.get(&key) {
            return Ok(slot);
        }
        let texture = crate::decode_texture(&model.textures[index].ktx2)?;
        let slot = self.textures.len() as u32;
        self.textures.push(texture);
        self.slots.insert(key, slot);
        Ok(slot)
    }

    /// The model material with every texture slot; emissive scaled by the entity.
    fn model_material(
        &mut self,
        material: &mut Material,
        model: &Model,
        m: &ara_runtime::model::ModelMaterial,
        color: [f32; 4],
        emissive: [f32; 3],
        emissive_factor: Option<[f32; 3]>,
    ) -> Result<()> {
        for (out, (a, b)) in material
            .base_color
            .iter_mut()
            .zip(m.base_color.iter().zip(color))
        {
            *out = a * b;
        }
        let factor = emissive_factor.unwrap_or(m.emissive);
        material.emissive_roughness = [
            factor[0] * emissive[0],
            factor[1] * emissive[1],
            factor[2] * emissive[2],
            m.roughness,
        ];
        material.surface = [m.metallic, m.normal_scale, 1.0, m.alpha_cutoff];
        material.flags[0] = m.alpha_mode;
        for (slot, texture) in [
            m.base_color_texture,
            m.normal_texture,
            m.metallic_roughness_texture,
            m.emissive_texture,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(t) = texture {
                material.textures[slot] = self.texture(model, t)?;
            }
        }
        Ok(())
    }
}

fn solid(rgba: [u8; 4], srgb: bool) -> Texture {
    Texture {
        size: [1, 1],
        rgba: rgba.to_vec(),
        srgb,
    }
}

/// Every local light (the fidelity view has no 64-light cap).
fn lights(frame: &RenderFrame) -> Vec<scene::Light> {
    frame
        .lights
        .iter()
        .map(|l| {
            let [x, y, z] = l.position;
            let [dx, dy, dz] = l.direction;
            let spot = l.kind == LightKind::Spot;
            scene::Light {
                position_range: [x, y, z, l.range],
                color_lumens: [l.color[0], l.color[1], l.color[2], l.lumens],
                direction_outer: [
                    dx,
                    dy,
                    dz,
                    if spot {
                        l.cone[1].to_radians().cos()
                    } else {
                        0.0
                    },
                ],
                parameters: if spot {
                    [1.0, l.cone[0].to_radians().cos(), 0.0, 0.0]
                } else {
                    [0.0; 4]
                },
            }
        })
        .collect()
}
