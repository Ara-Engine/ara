//! Terrain and foliage on the Renderlab path, first pass: ordinary meshes.
//!
//! Each resident terrain tile is the world-space mesh Ara's presenter already builds
//! (`ara_runtime_render::terrain_mesh`), at full resolution. Its colour is the base
//! colour factor of each of the terrain's splat-layer materials, blended by the tile's
//! splat weights into one texture; a terrain without layer materials has Ara's default
//! white surface. Each foliage instance of
//! a resident tile draws the parts of its rule's model. What that leaves out against
//! `ara-landscape`: distance levels of detail, the layered terrain materials and their
//! virtual texture, and wind. Tiles and foliage follow tile residency, so streaming
//! still decides what exists.
use crate::{gaps::Gaps, scene, texture};
use anyhow::Result;
use ara_runtime::extract::Draw;
use ara_runtime_render::{
    ExternalView,
    terrain_mesh::{TerrainMeshes, TileMesh},
};
use glam34::{Mat4 as AraMat4, Quat, Vec3 as AraVec3};
use renderlab::{
    asset::{TextureRole, Wrap},
    engine::{
        Engine, Mat4, MaterialDesc, MaterialId, MeshData, MeshId, ObjectId, Sampler, TextureId,
        TextureRef, Vertex,
    },
};
use std::collections::{HashMap, HashSet};

/// Foliage part draws kept at most: each is one Renderlab object.
const MAX_FOLIAGE_DRAWS: usize = 60_000;

/// Triangles a draw's mesh holds (shapes: a few hundred at most).
fn triangles(draw: &Draw) -> usize {
    match &draw.geometry {
        ara_runtime::extract::Geometry::Model { model, primitive } => model
            .primitives
            .get(*primitive)
            .map_or(0, |p| p.indices.len() / 3),
        ara_runtime::extract::Geometry::Shape { .. } => 1024,
    }
}

/// Instances Renderlab's non-RT virtual shadows can address: they pack an instance and a
/// triangle of the largest mesh into one 32-bit id, so the largest mesh (doubled, for the
/// level-of-detail triangles Renderlab appends) leaves the rest of the bits to instances.
fn packed_instances(largest_triangles: usize) -> usize {
    let bits = usize::BITS - (largest_triangles.max(1) * 2).leading_zeros();
    (1usize << 32u32.saturating_sub(bits).min(31)).saturating_sub(2)
}
/// Marks foliage render identities apart from entity and terrain-tile identities.
const FOLIAGE_ENTITY: u64 = 0x7f01_0000_0000_0000;
const FNV: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// One tile's Renderlab resources.
struct Tile {
    object: ObjectId,
    mesh: MeshId,
    material: MaterialId,
    texture: Option<TextureId>,
}

/// The terrain tiles placed in Renderlab and the frame's foliage as draws.
#[derive(Default)]
pub(crate) struct Terrain {
    meshes: TerrainMeshes,
    /// Placed tiles by their content key.
    tiles: HashMap<u64, Tile>,
    /// Foliage part draws of the resident tiles, for `Scene::sync`.
    foliage: Vec<Draw>,
    /// What `foliage` was built from; a new value means every draw may have changed.
    foliage_key: u64,
}

impl Terrain {
    /// Places the frame's resident tiles, at most `budget` (each is one of Renderlab's
    /// mesh-material groups), and rebuilds the foliage draws when their inputs
    /// changed. With `packed_ids` (the non-RT path: the largest mesh the scene has
    /// uploaded), foliage also stays within the instances its shadows can address
    /// (`packed_instances`). Returns the number of tiles placed and the largest mesh
    /// the foliage cap assumed.
    pub fn sync(
        &mut self,
        engine: &mut Engine,
        view: &ExternalView<'_>,
        budget: usize,
        packed_ids: Option<usize>,
        gaps: &mut Gaps,
    ) -> Result<(usize, usize)> {
        let Some(terrain) = view.frame.terrain.as_ref() else {
            self.clear(engine)?;
            return Ok((0, 0));
        };
        let mut coords = terrain.tiles.clone();
        coords.sort();
        coords.dedup();
        let layers = Layers::resolve(view, gaps);
        let wanted: Vec<_> = coords
            .iter()
            .filter_map(|coord| {
                let mesh = self.meshes.tile(&terrain.data, *coord)?;
                Some((mesh.key ^ layers.key, mesh, terrain.data.tiles.get(coord)?))
            })
            .collect();
        if wanted.len() > budget {
            gaps.limit(
                "mesh-material groups per frame (terrain tiles)",
                &format!(
                    "{} of {} resident terrain tiles are not drawn",
                    wanted.len() - budget,
                    wanted.len()
                ),
            );
        }
        let keep: HashSet<u64> = wanted.iter().take(budget).map(|tile| tile.0).collect();
        let gone: Vec<u64> = self
            .tiles
            .keys()
            .filter(|key| !keep.contains(key))
            .copied()
            .collect();
        for key in gone {
            if let Some(tile) = self.tiles.remove(&key) {
                release(engine, tile)?;
            }
        }
        for (key, mesh, tile) in wanted.iter().take(budget) {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.tiles.entry(*key) {
                slot.insert(place(engine, mesh, &tile.weights, &layers)?);
            }
        }

        // Foliage: rebuilt when the terrain generation, the resident tiles or the
        // resolved parts change.
        let mut key = fnv(
            FNV,
            &(std::sync::Arc::as_ptr(&terrain.data) as usize).to_le_bytes(),
        );
        for coord in &coords {
            key = fnv(fnv(key, &coord.x.to_le_bytes()), &coord.z.to_le_bytes());
        }
        for (asset, parts) in view.foliage_parts {
            key = fnv(fnv(key, asset.as_bytes()), &parts.len().to_le_bytes());
            for part in parts {
                key = fnv(key, &part.entity.to_le_bytes());
            }
        }
        let largest = packed_ids.map_or(0, |uploaded| {
            wanted
                .iter()
                .map(|(_, mesh, _)| mesh.source.indices.len() / 3)
                .chain(view.frame.draws.iter().map(triangles))
                .chain(view.foliage_parts.values().flatten().map(triangles))
                .fold(uploaded, usize::max)
        });
        let cap = if packed_ids.is_some() {
            packed_instances(largest)
                .saturating_sub(view.frame.draws.len() + budget)
                .min(MAX_FOLIAGE_DRAWS)
        } else {
            MAX_FOLIAGE_DRAWS
        };
        key = fnv(key, &cap.to_le_bytes());
        if key != self.foliage_key {
            self.foliage_key = key;
            self.rebuild_foliage(
                view,
                &coords.iter().map(|c| (c.x, c.z)).collect(),
                cap,
                gaps,
            );
        }
        Ok((self.tiles.len(), largest))
    }

    /// The foliage draws and the key of what they were built from.
    pub fn foliage(&self) -> (&[Draw], u64) {
        (&self.foliage, self.foliage_key)
    }

    fn clear(&mut self, engine: &mut Engine) -> Result<()> {
        for (_, tile) in self.tiles.drain() {
            release(engine, tile)?;
        }
        if !self.foliage.is_empty() || self.foliage_key != 0 {
            self.foliage.clear();
            self.foliage_key = 0;
        }
        Ok(())
    }

    fn rebuild_foliage(
        &mut self,
        view: &ExternalView<'_>,
        resident: &HashSet<(i32, i32)>,
        cap: usize,
        gaps: &mut Gaps,
    ) {
        self.foliage.clear();
        let Some(terrain) = view.frame.terrain.as_ref() else {
            return;
        };
        let data = &terrain.data;
        let (origin, width) = (data.header.origin, data.header.tile_width());
        let (mut unresolved, mut over) = (0usize, 0usize);
        for instance in &data.foliage {
            let [x, y, z, scale] = instance.position_scale;
            let coord = (
                ((x - origin[0]) / width).floor() as i32,
                ((z - origin[1]) / width).floor() as i32,
            );
            if !resident.contains(&coord) {
                continue;
            }
            let rule = instance.identity[3];
            let parts = data
                .foliage_rules
                .get(rule as usize)
                .and_then(|entry| view.foliage_parts.get(&entry.rule.asset.0));
            let Some(parts) = parts else {
                unresolved += 1;
                continue;
            };
            let placed = AraMat4::from_scale_rotation_translation(
                AraVec3::splat(scale),
                Quat::from_rotation_y(instance.rotation_wind[0]),
                AraVec3::new(x, y, z),
            );
            for (index, part) in parts.iter().enumerate() {
                if self.foliage.len() == cap {
                    over += 1;
                    continue;
                }
                let mut entity = fnv(FNV, &instance.identity[0].to_le_bytes());
                entity = fnv(entity, &instance.identity[1].to_le_bytes());
                entity = fnv(
                    fnv(entity, &rule.to_le_bytes()),
                    &(index as u32).to_le_bytes(),
                );
                self.foliage.push(Draw {
                    entity: FOLIAGE_ENTITY | (entity & 0x0000_ffff_ffff_ffff),
                    world: placed * part.world,
                    ..part.clone()
                });
            }
        }
        if unresolved > 0 {
            gaps.unsupported(
                "foliage rules whose model is not resolved",
                &format!("{unresolved} foliage instances are not drawn"),
            );
        }
        if over > 0 {
            gaps.limit(
                "foliage draws the bridge keeps",
                &format!(
                    "{over} foliage parts beyond {cap} are not drawn ({MAX_FOLIAGE_DRAWS} at most; fewer on the non-RT path, whose shadows pack instance and triangle into 32 bits)"
                ),
            );
        }
    }
}

/// The terrain's splat layers as the bridge draws them.
struct Layers {
    /// Linear base colour factor per splat channel; `None` without layer materials.
    colours: Option<[[f32; 3]; 4]>,
    roughness: f32,
    /// Changes when the colours do, so tiles are placed again.
    key: u64,
}

impl Layers {
    fn resolve(view: &ExternalView<'_>, gaps: &mut Gaps) -> Self {
        let references = view
            .frame
            .terrain
            .as_ref()
            .map_or(&[][..], |terrain| &terrain.data.header.materials[..]);
        if references.is_empty() {
            // As Ara's default surface: white, half rough.
            return Self {
                colours: None,
                roughness: 0.5,
                key: 0,
            };
        }
        gaps.unsupported(
            "terrain layer maps",
            "each layer shows its base colour factor, blended by the splat weights",
        );
        let mut colours = [[1.; 3]; 4];
        let (mut roughness, mut key) = (0., FNV);
        for (colour, reference) in colours.iter_mut().zip(references) {
            let material = view
                .terrain_materials
                .get(&reference.0)
                .map(|m| &m.material);
            if let Some(material) = material {
                colour.copy_from_slice(&material.base_color[..3]);
            }
            roughness += material.map_or(0.5, |m| m.roughness) / references.len().min(4) as f32;
            for channel in colour {
                key = fnv(key, &channel.to_bits().to_le_bytes());
            }
        }
        Self {
            colours: Some(colours),
            roughness: roughness.clamp(0.045, 1.),
            key,
        }
    }
}

/// Uploads one tile: its mesh and a material coloured by its splat layers.
fn place(
    engine: &mut Engine,
    tile: &TileMesh,
    weights: &[[u8; 4]],
    layers: &Layers,
) -> Result<Tile> {
    let vertices: Vec<Vertex> = tile.source.vertices.iter().map(scene::vertex).collect();
    let mesh = engine.create_mesh(&MeshData {
        vertices: &vertices,
        indices: &tile.source.indices,
    })?;
    // One texel per height sample, as the tile mesh's coordinates address them.
    let texture = match layers.colours {
        Some(colours) if weights.len() as u32 == tile.texture.size[0] * tile.texture.size[1] => {
            let mut rgba = Vec::with_capacity(weights.len() * 4);
            for sample in weights {
                let total = sample.iter().map(|w| f32::from(*w)).sum::<f32>().max(1.);
                for channel in 0..3 {
                    let linear: f32 = colours
                        .iter()
                        .zip(sample)
                        .map(|(colour, weight)| colour[channel] * f32::from(*weight) / total)
                        .sum();
                    rgba.push(texture::linear_to_srgb(linear));
                }
                rgba.push(255);
            }
            Some(engine.create_texture(texture::chain(
                tile.texture.size,
                rgba,
                TextureRole::BaseColorSrgb,
            ))?)
        }
        _ => None,
    };
    let material = engine.create_material(&MaterialDesc {
        base_color: [1.; 4],
        metallic: 0.,
        roughness: layers.roughness,
        double_sided: true,
        base_color_texture: texture.map(|texture| TextureRef {
            texture,
            // No wrap onto the tile's far edge.
            sampler: Sampler {
                wrap_u: Wrap::Clamp,
                wrap_v: Wrap::Clamp,
                ..Default::default()
            },
        }),
        ..Default::default()
    })?;
    let object = engine.spawn(mesh, material, Mat4::IDENTITY)?;
    Ok(Tile {
        object,
        mesh,
        material,
        texture,
    })
}

fn release(engine: &mut Engine, tile: Tile) -> Result<()> {
    engine.despawn(tile.object)?;
    engine.remove_material(tile.material)?;
    engine.remove_mesh(tile.mesh)?;
    if let Some(texture) = tile.texture {
        engine.remove_texture(texture)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_ids_leave_the_bits_the_largest_mesh_does_not_need() {
        // 32,768 triangles, doubled: 17 bits, so 15 bits of instances.
        assert_eq!(packed_instances(32_768), (1 << 15) - 2);
        assert_eq!(packed_instances(1), (1 << 30) - 2);
    }
}
