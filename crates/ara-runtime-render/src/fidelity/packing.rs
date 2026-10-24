//! Transitional atlas and material-budget packing for the current GPU heap.
use super::*;

impl Builder {
    /// The scene: one mesh per material group, textures within `limit`.
    pub(super) fn finish(mut self, limit: usize, stats: &mut Stats) -> Result<Scene> {
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
        stats.atlased_materials = atlas::pack(
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
            sun_direction: glam30::Vec3::Y,
            sun_lux: glam30::Vec3::ZERO,
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
