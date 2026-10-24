use super::*;
use ara_runtime::extract::TerrainView;
use glam::{Mat4, Quat, Vec3};

impl Builder {
    pub(super) fn terrain(&mut self, view: &TerrainView, content: &Content) -> Result<()> {
        let data = &view.data;
        let header = &data.header;
        let mut splat = Material::default();
        for (channel, reference) in header.materials.iter().enumerate() {
            let source = content
                .terrain_materials
                .get(&reference.0)
                .with_context(|| format!("unresolved terrain material {}", reference.0))?;
            let mut material = Material::default();
            self.source_material(&mut material, crate::material::Source::Native(source))?;
            splat.extras.layer_materials[channel] = self.add_material(material) as u32;
        }
        splat.extras.terrain_layers = header.materials.len() as u32;
        let material = self.add_material(splat);
        let cells = header.cells as usize;
        let stride = cells + 1;
        for coord in &view.tiles {
            let tile = data
                .tiles
                .get(coord)
                .context("resident terrain tile missing")?;
            let mut vertices = Vec::with_capacity(stride * stride);
            for z in 0..=cells {
                for x in 0..=cells {
                    let px = header.origin[0]
                        + coord.x as f32 * header.tile_width()
                        + x as f32 * header.spacing;
                    let pz = header.origin[1]
                        + coord.z as f32 * header.tile_width()
                        + z as f32 * header.spacing;
                    let index = z * stride + x;
                    let n = Vec3::from_array(
                        data.surface([px, pz]).map_or([0.0, 1.0, 0.0], |v| v.normal),
                    );
                    let t = (Vec3::X - n * n.x).normalize_or(Vec3::X);
                    vertices.push(render::Vertex {
                        position: [px, tile.heights[index], pz],
                        normal: n.to_array(),
                        uv: [px, pz],
                        uv1: [px, pz],
                        tangent: t.extend(-1.0).to_array(),
                        terrain_weights: u32::from_le_bytes(tile.weights[index]),
                        reserved: 0,
                    });
                }
            }
            let mut indices = Vec::with_capacity(cells * cells * 6);
            for z in 0..cells {
                for x in 0..cells {
                    let i = z * stride + x;
                    if tile.holes[i] != 0 {
                        continue;
                    }
                    let a = i as u32;
                    let b = (i + stride) as u32;
                    indices.extend([a, b, a + 1, a + 1, b, b + 1]);
                }
            }
            if indices.is_empty() {
                continue;
            }
            let mut hash = Sha256::new();
            hash.update(b"terrain");
            hash.update(bytemuck::cast_slice(&vertices));
            hash.update(bytemuck::cast_slice(&indices));
            let key: [u8; 32] = hash.finalize().into();
            let mut bounds = [
                glam30::Vec3::splat(f32::INFINITY),
                glam30::Vec3::splat(f32::NEG_INFINITY),
            ];
            for v in &vertices {
                let p = glam30::Vec3::from_array(v.position);
                bounds[0] = bounds[0].min(p);
                bounds[1] = bounds[1].max(p);
            }
            let geometry = self.scene.geometry.len();
            self.scene.geometry.push(render::Geometry {
                key,
                vertices,
                lods: vec![render::Lod {
                    indices,
                    error: 0.0,
                }],
                bounds,
                deforming: false,
                revision: 0,
            });
            self.scene.instances.push(Instance {
                entity: u64::from_le_bytes(key[..8].try_into()?),
                name: format!("Terrain {},{}", coord.x, coord.z),
                geometry,
                material,
                world: glam30::Mat4::IDENTITY,
                previous_world: glam30::Mat4::IDENTITY,
            });
        }
        let resident: std::collections::BTreeSet<_> = view.tiles.iter().copied().collect();
        let empty = RenderFrame::default();
        for instance in &data.foliage {
            let p = instance.position_scale;
            let coord = ara_terrain::TileCoord {
                x: ((p[0] - header.origin[0]) / header.tile_width()).floor() as i32,
                z: ((p[2] - header.origin[1]) / header.tile_width()).floor() as i32,
            };
            if !resident.contains(&coord) {
                continue;
            }
            let metadata = data
                .foliage_rules
                .get(instance.identity[3] as usize)
                .context("invalid foliage rule index")?;
            let asset = &metadata.rule.asset;
            let parts = content
                .foliage_parts
                .get(&asset.0)
                .with_context(|| format!("unresolved foliage asset {}", asset.0))?;
            let p = instance.position_scale;
            let world = Mat4::from_scale_rotation_translation(
                Vec3::splat(p[3]),
                Quat::from_rotation_y(instance.rotation_wind[0]),
                Vec3::from_slice(&p),
            );
            let identity = u64::from(instance.identity[0]) | u64::from(instance.identity[1]) << 32;
            for part in parts {
                let mut hash = Sha256::new();
                hash.update(metadata.id.as_bytes());
                hash.update(identity.to_le_bytes());
                hash.update(part.entity.to_le_bytes());
                let key: [u8; 32] = hash.finalize().into();
                self.draw(
                    part,
                    &empty,
                    world * part.world,
                    u64::from_le_bytes(key[..8].try_into()?),
                )?;
            }
        }
        Ok(())
    }
}
