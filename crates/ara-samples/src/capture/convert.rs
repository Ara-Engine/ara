//! A presentation frame as a fidelity scene snapshot: every draw with its full PBR
//! texture set (base colour, normal, metal/roughness, emissive), terrain tiles under
//! the look's ground material, each foliage rule's instances drawn with the rule's
//! own model, all local lights, the sun and the sky.
//!
//! The fidelity view caps a snapshot at 96 textures and 4096 meshes (one bindless
//! heap of 128 sampled images; no instancing). Real content exceeds both, so the
//! snapshot is a still: geometry is merged per material (vertices are world space
//! already), meshes use the coarsest cooked LOD within a world-space error, and the
//! texture budget keeps the maps that cover the most surface; dropped maps fold into
//! their material's factors (mean colour, mean roughness/metalness, flat normal).
use super::{
    look::{Detail, Ground, Look, Sky},
    world::World,
};
use crate::lock::Lock;
use anyhow::{Context, Result};
use ara_render::scene::{self, Material, Mesh, Scene, Texture, Vertex};
use ara_runtime::{
    components::LightKind,
    extract::{Geometry, RenderFrame},
    model::{Model, Primitive},
};
use glam34::{Mat4, Quat, Vec3};
use std::{collections::BTreeMap, path::Path};

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

/// Builds the snapshot. `lock` and `root` resolve look texture sets and HDRIs.
pub fn scene(
    world: &mut World,
    frame: &RenderFrame,
    look: &Look,
    lock: &Lock,
    root: &Path,
) -> Result<(Scene, Stats)> {
    let mut out = Builder::default();
    let details: BTreeMap<&str, &Detail> =
        look.details.iter().map(|d| (d.asset.as_str(), d)).collect();
    for draw in &frame.draws {
        let palette = draw.skin.clone().map(|r| &frame.palettes[r]);
        let mut material = Material {
            base_color: draw.color,
            emissive_roughness: [
                draw.emissive[0],
                draw.emissive[1],
                draw.emissive[2],
                draw.roughness,
            ],
            surface: [draw.metallic, 1.0, 1.0, 0.5],
            textures: [0, 1, 0, 0],
            flags: [u32::from(draw.color[3] < 1.0) * 2, 0, 0, 0],
        };
        let mut vertices = Vec::new();
        let indices = match &draw.geometry {
            Geometry::Shape { shape, size } => {
                let source = ara_runtime_render::geometry::shape(*shape, *size);
                ara_runtime_render::geometry::transform(
                    &source.vertices,
                    draw.world,
                    palette,
                    &mut vertices,
                );
                source.indices
            }
            Geometry::Model { model, primitive } => {
                let p = &model.primitives[*primitive];
                ara_runtime_render::geometry::transform(
                    &p.vertices,
                    draw.world,
                    palette,
                    &mut vertices,
                );
                if let Some(m) = p.material.and_then(|m| model.materials.get(m)) {
                    out.model_material(&mut material, model, m, draw.color, draw.emissive)?;
                    if let Some(d) = details.get(model.asset.as_str()) {
                        detail(&mut material, d)?;
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
        if let Some(ground) = &look.terrain {
            out.terrain(&view.data, ground, lock, root)?;
        }
        out.foliage(world, &view.data)?;
    }
    let names = std::mem::take(&mut out.names);
    let mut stats = std::mem::take(&mut out.stats);
    let mut scene = out.finish(TEXTURE_LIMIT, &mut stats)?;
    scene.lights = lights(frame);
    let (direction, lux) = frame.sun.unwrap_or(([0.3, 1.0, 0.2], [0.0; 3]));
    scene.sun_direction = glam::Vec3::from_array(direction).normalize();
    scene.sun_lux = glam::Vec3::from_array(lux);
    match &look.sky {
        Sky::Hdri {
            asset,
            intensity,
            rotation,
        } => {
            let entry = lock
                .get(asset)
                .with_context(|| format!("no HDRI {asset}"))?;
            let file = entry.entry(&[".hdr"]).context("HDRI file")?;
            let (size, pixels) = hdri(&entry.dir(root).join(&file.path), *intensity, *rotation)?;
            scene.environment_size = size;
            scene.environment = pixels;
        }
        Sky::Atmosphere { .. } | Sky::Scene => {
            let (size, pixels) = ara_runtime_render::sky(&frame.environment);
            scene.environment_size = size;
            scene.environment = pixels;
        }
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
        let texture = ara_runtime_render::decode_texture(&model.textures[index].ktx2)?;
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
    ) -> Result<()> {
        for (out, (a, b)) in material
            .base_color
            .iter_mut()
            .zip(m.base_color.iter().zip(color))
        {
            *out = a * b;
        }
        material.emissive_roughness = [
            m.emissive[0] * emissive[0],
            m.emissive[1] * emissive[1],
            m.emissive[2] * emissive[2],
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

    /// A texture set from the lock as a material (colour, normal, ARM).
    fn texture_set(
        &mut self,
        lock: &Lock,
        root: &Path,
        id: &str,
        tint: [f32; 3],
    ) -> Result<(Material, [f32; 2])> {
        let set = lock
            .get(id)
            .with_context(|| format!("no texture set {id}"))?;
        let meters = set.meters.unwrap_or([2.0, 2.0]);
        let mut material = Material {
            base_color: [tint[0], tint[1], tint[2], 1.0],
            emissive_roughness: [0.0, 0.0, 0.0, 1.0],
            surface: [1.0, 1.0, 1.0, 0.5],
            textures: [0, 1, 0, 0],
            flags: [0; 4],
        };
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
            material.textures[slot] = self.textures.len() as u32;
            self.textures.push(Texture {
                size: [image.width(), image.height()],
                rgba: image.into_raw(),
                srgb,
            });
        }
        Ok((material, meters))
    }

    /// Terrain tiles: each cell takes the ground or the rock material, chosen from
    /// the painted rock weight, the slope and a little noise (ragged outcrops).
    fn terrain(
        &mut self,
        data: &ara_runtime::terrain::TerrainData,
        ground: &Ground,
        lock: &Lock,
        root: &Path,
    ) -> Result<()> {
        let mut sets = vec![self.texture_set(lock, root, &ground.texture, ground.tint)?];
        if let Some(rock) = &ground.rock {
            sets.push(self.texture_set(lock, root, rock, ground.rock_tint)?);
        }
        let header = &data.header;
        let cells = header.cells as usize;
        let width = header.tile_width();
        let mut out: Vec<(Vec<Vertex>, Vec<u32>)> = vec![(Vec::new(), Vec::new()); sets.len()];
        for (coord, tile) in &data.tiles {
            let origin = [
                header.origin[0] + coord.x as f32 * width,
                header.origin[1] + coord.z as f32 * width,
            ];
            let sample = |x: usize, z: usize| {
                let p = [
                    origin[0] + x as f32 * header.spacing,
                    origin[1] + z as f32 * header.spacing,
                ];
                let i = z * (cells + 1) + x;
                let normal = data.surface(p).map_or([0.0, 1.0, 0.0], |s| s.normal);
                (
                    Vec3::new(p[0], tile.heights[i], p[1]),
                    Vec3::from_array(normal),
                    f32::from(tile.weights[i][1]) / 255.0,
                )
            };
            for z in 0..cells {
                for x in 0..cells {
                    if tile.holes.get(z * (cells + 1) + x).copied().unwrap_or(0) != 0 {
                        continue;
                    }
                    let corners = [
                        sample(x, z),
                        sample(x + 1, z),
                        sample(x, z + 1),
                        sample(x + 1, z + 1),
                    ];
                    let rock = corners.iter().map(|c| c.2).sum::<f32>() / 4.0;
                    let up = corners.iter().map(|c| c.1.y).sum::<f32>() / 4.0;
                    let h = ara_foliage::hash(
                        ((corners[0].0.x as i64 as u64) << 32) ^ (corners[0].0.z as i64 as u64),
                    );
                    let noise = (h >> 40) as f32 / (1u64 << 24) as f32;
                    let score = rock * 0.8 + (1.0 - up) * 2.2 + (noise - 0.5) * 0.35;
                    let which = usize::from(sets.len() > 1 && score > 0.55);
                    let meters = sets[which].1;
                    let (vertices, indices) = &mut out[which];
                    let base = vertices.len() as u32;
                    for (p, n, _) in corners {
                        let t = (Vec3::X - n * n.x).normalize_or(Vec3::X);
                        vertices.push(Vertex {
                            position: p.to_array(),
                            normal: n.to_array(),
                            uv: [p.x / meters[0], p.z / meters[1]],
                            tangent: t.extend(1.0).to_array(),
                        });
                    }
                    indices.extend([base, base + 2, base + 1, base + 1, base + 2, base + 3]);
                }
            }
        }
        for ((material, _), (vertices, indices)) in sets.into_iter().zip(out) {
            self.stats.finest_triangles += indices.len() / 3;
            self.add("Terrain", vertices, indices, material);
        }
        Ok(())
    }

    /// Each enabled foliage rule scattered exactly as the runtime does, drawn with the
    /// rule's model (the runtime renderers draw procedural stand-ins instead).
    fn foliage(
        &mut self,
        world: &mut World,
        data: &ara_runtime::terrain::TerrainData,
    ) -> Result<()> {
        let Some(doc) = world
            .workspace
            .docs()
            .find(|(_, d)| d.kind == "foliage")
            .map(|(_, d)| d.clone())
        else {
            return Ok(());
        };
        let mut rules = Vec::new();
        let mut exclusions = Vec::new();
        for (id, record) in doc.records() {
            let value = serde_json::Value::Object(record.clone());
            if id.as_str().starts_with("x_") {
                exclusions.push(serde_json::from_value::<ara_foliage::Exclusion>(value)?);
            } else if id.as_str().starts_with("r_") {
                let rule: ara_foliage::Rule = serde_json::from_value(value)?;
                if rule.enabled {
                    rules.push(rule);
                }
            }
        }
        let width = data.header.tile_width();
        let surface = |p: [f32; 2]| data.surface(p);
        for (index, rule) in rules.iter().enumerate() {
            let asset = ara_runtime::assets::asset_id(&rule.asset)?.to_owned();
            let model = world.model(&asset)?;
            let mut materials = Vec::new();
            for primitive in &model.primitives {
                let mut material = Material::default();
                if let Some(m) = primitive.material.and_then(|m| model.materials.get(m)) {
                    self.model_material(&mut material, &model, m, [1.0; 4], [0.0; 3])?;
                }
                materials.push(material);
            }
            for coord in data.tiles.keys() {
                let min = [
                    data.header.origin[0] + coord.x as f32 * width,
                    data.header.origin[1] + coord.z as f32 * width,
                ];
                let bounds = ara_foliage::Bounds {
                    min,
                    max: [min[0] + width, min[1] + width],
                };
                let instances =
                    ara_foliage::scatter(index as u64, rule, bounds, &surface, &exclusions)
                        .map_err(|d| anyhow::anyhow!("foliage: {}", d.message))?;
                self.stats.foliage_instances += instances.len();
                for instance in instances {
                    let [x, y, z, s] = instance.position_scale;
                    let matrix = Mat4::from_scale_rotation_translation(
                        Vec3::splat(s),
                        Quat::from_rotation_y(instance.rotation_wind[0]),
                        Vec3::new(x, y, z),
                    );
                    for (primitive, material) in model.primitives.iter().zip(&materials) {
                        let mut vertices = Vec::new();
                        ara_runtime_render::geometry::transform(
                            &primitive.vertices,
                            matrix,
                            None,
                            &mut vertices,
                        );
                        self.stats.finest_triangles += primitive.indices.len() / 3;
                        let indices = lod(primitive, extent(&model) * s, FOLIAGE_ERROR).to_vec();
                        self.add(&rule.name, vertices, indices, *material);
                    }
                }
            }
        }
        Ok(())
    }

    /// The scene: one mesh per material group, textures within `limit`.
    fn finish(mut self, limit: usize, stats: &mut Stats) -> Result<Scene> {
        stats.textures_wanted = self.textures.len();
        // Importance of each texture: world-space area of the surfaces sampling it,
        // weighted by how much the map shows (colour > emissive > normal > ARM).
        let mut area = vec![0.0f64; self.materials.len()];
        for (&material, group) in &self.groups {
            for t in group.indices.as_chunks::<3>().0 {
                let p = |i: u32| Vec3::from_array(group.vertices[i as usize].position);
                area[material] +=
                    f64::from((p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).length() * 0.5);
            }
        }
        let atlas_start = self.textures.len();
        let mut vertices: BTreeMap<usize, &mut Vec<Vertex>> = self
            .groups
            .iter_mut()
            .map(|(m, g)| (*m, &mut g.vertices))
            .collect();
        stats.atlased_materials = super::atlas::pack(
            &mut self.textures,
            &mut self.materials,
            &mut vertices,
            &area,
        )?;
        drop(vertices);
        let mut importance = vec![0.0f64; self.textures.len()];
        let mut referenced = std::collections::BTreeSet::new();
        for (m, material) in self.materials.iter().enumerate() {
            for (slot, weight) in [(0, 4.0), (1, 1.5), (2, 1.0), (3, 3.0)] {
                let t = material.textures[slot] as usize;
                if t > 1 {
                    referenced.insert(t);
                    importance[t] += if t >= atlas_start {
                        f64::INFINITY
                    } else {
                        area[m].sqrt() * weight
                    };
                }
            }
        }
        let mut order: Vec<usize> = referenced.into_iter().collect();
        order.sort_by(|a, b| importance[*b].total_cmp(&importance[*a]));
        let keep: std::collections::BTreeSet<usize> = order
            .iter()
            .take(limit.saturating_sub(2))
            .copied()
            .collect();
        // Fold dropped maps into factors.
        let means: Vec<[f32; 4]> = self.textures.iter().map(mean).collect();
        for material in &mut self.materials {
            let t = material.textures.map(|t| t as usize);
            if t[0] > 1 && !keep.contains(&t[0]) {
                for (c, m) in material.base_color.iter_mut().zip(means[t[0]]).take(3) {
                    *c = (*c * m).clamp(0.0, 1.0);
                }
                material.textures[0] = 0;
            }
            if t[1] > 1 && !keep.contains(&t[1]) {
                material.textures[1] = 1;
            }
            if t[2] > 1 && !keep.contains(&t[2]) {
                material.emissive_roughness[3] *= means[t[2]][1];
                material.surface[0] *= means[t[2]][2];
                material.textures[2] = 0;
            }
            if t[3] > 1 && !keep.contains(&t[3]) {
                for (c, m) in material
                    .emissive_roughness
                    .iter_mut()
                    .zip(means[t[3]])
                    .take(3)
                {
                    *c *= m;
                }
                material.textures[3] = 0;
            }
        }
        // Compact the kept textures.
        let mut remap = vec![0u32; self.textures.len()];
        let mut textures = Vec::new();
        for (i, texture) in self.textures.into_iter().enumerate() {
            if i < 2 || keep.contains(&i) {
                remap[i] = textures.len() as u32;
                textures.push(texture);
            }
        }
        for material in &mut self.materials {
            for t in &mut material.textures {
                *t = remap[*t as usize];
            }
        }
        let meshes = self
            .groups
            .into_iter()
            .enumerate()
            .map(|(i, (material, group))| Mesh {
                entity: 0x5a3e_0000 + i as u64,
                name: group.name,
                vertices: group.vertices,
                indices: group.indices,
                material,
            })
            .collect();
        Ok(Scene {
            meshes,
            materials: self.materials,
            textures,
            lights: Vec::new(),
            sun_direction: glam::Vec3::Y,
            sun_lux: glam::Vec3::ZERO,
            environment: Vec::new(),
            environment_size: [0; 2],
        })
    }
}

/// Mean texel: linear RGB for sRGB textures (so folded factors stay physical), raw
/// channels otherwise.
fn mean(t: &Texture) -> [f32; 4] {
    let mut sum = [0.0f64; 4];
    let step = (t.rgba.len() / 4 / 4096).max(1);
    let mut n = 0.0;
    for px in t.rgba.as_chunks::<4>().0.iter().step_by(step) {
        for c in 0..4 {
            let v = f64::from(px[c]) / 255.0;
            sum[c] += if t.srgb && c < 3 {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            } else {
                v
            };
        }
        n += 1.0;
    }
    sum.map(|s| (s / n) as f32)
}

/// Encodes look material layers into the snapshot's reserved words.
fn detail(material: &mut Material, d: &Detail) -> Result<()> {
    ara_render_fidelity::material::MaterialDetail {
        clearcoat: d.clearcoat,
        sheen: d.sheen,
        transmission: d.transmission,
        ..Default::default()
    }
    .apply(material)
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
