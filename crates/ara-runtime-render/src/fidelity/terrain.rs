//! Terrain and foliage conversion at the scene-upload boundary.
use super::*;
use glam::{Mat4, Quat};

impl Builder {
    /// Terrain layers resolved by the caller, in splat-channel order.
    pub(super) fn terrain(
        &mut self,
        data: &ara_runtime::terrain::TerrainData,
        layers: Vec<TerrainLayer>,
    ) -> Result<()> {
        let sets: Vec<_> = layers
            .into_iter()
            .map(|layer| {
                let mut material = layer.material;
                for (slot, texture) in layer.textures.into_iter().enumerate() {
                    if let Some(texture) = texture {
                        material.textures[slot] = self.textures.len() as u32;
                        self.textures.push(texture);
                    }
                }
                (material, layer.meters)
            })
            .collect();
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
    pub(super) fn foliage(
        &mut self,
        foliage: &[Foliage],
        exclusions: &[ara_foliage::Exclusion],
        data: &ara_runtime::terrain::TerrainData,
    ) -> Result<()> {
        let width = data.header.tile_width();
        let surface = |p: [f32; 2]| data.surface(p);
        for (index, entry) in foliage.iter().enumerate() {
            let rule = &entry.rule;
            if !rule.enabled {
                continue;
            }
            let model = &entry.model;
            let mut materials = Vec::new();
            for primitive in &model.primitives {
                let mut material = Material::default();
                if let Some(m) = primitive.material.and_then(|m| model.materials.get(m)) {
                    self.model_material(&mut material, model, m, [1.0; 4], [0.0; 3])?;
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
                    ara_foliage::scatter(index as u64, rule, bounds, &surface, exclusions)
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
                        crate::geometry::transform(
                            &primitive.vertices,
                            matrix,
                            None,
                            &mut vertices,
                        );
                        self.stats.finest_triangles += primitive.indices.len() / 3;
                        let indices = lod(primitive, extent(model) * s, FOLIAGE_ERROR).to_vec();
                        self.add(&rule.name, vertices, indices, *material);
                    }
                }
            }
        }
        Ok(())
    }
}
