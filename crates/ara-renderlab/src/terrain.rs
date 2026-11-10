//! Terrain and foliage on the Renderlab path, first pass: ordinary meshes.
//!
//! Each resident terrain tile is the world-space mesh Ara's presenter already builds
//! for presented frames (`ara_runtime_render::terrain_mesh`): full resolution, with a
//! base-colour texture tinted by the tile's splat weights. Each foliage instance of
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
    texture: TextureId,
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
    /// changed. Returns the number of tiles placed.
    pub fn sync(
        &mut self,
        engine: &mut Engine,
        view: &ExternalView<'_>,
        budget: usize,
        gaps: &mut Gaps,
    ) -> Result<usize> {
        let Some(terrain) = view.frame.terrain.as_ref() else {
            self.clear(engine)?;
            return Ok(0);
        };
        let mut coords = terrain.tiles.clone();
        coords.sort();
        coords.dedup();
        let wanted: Vec<_> = coords
            .iter()
            .filter_map(|coord| self.meshes.tile(&terrain.data, *coord))
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
        let keep: HashSet<u64> = wanted.iter().take(budget).map(|tile| tile.key).collect();
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
        for tile in wanted.iter().take(budget) {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.tiles.entry(tile.key) {
                slot.insert(place(engine, tile)?);
            }
        }

        // Foliage: rebuilt when the terrain generation, the resident tiles or the
        // resolved parts change.
        let mut key = fnv(FNV, &(std::sync::Arc::as_ptr(&terrain.data) as usize).to_le_bytes());
        for coord in &coords {
            key = fnv(fnv(key, &coord.x.to_le_bytes()), &coord.z.to_le_bytes());
        }
        for (asset, parts) in view.foliage_parts {
            key = fnv(fnv(key, asset.as_bytes()), &parts.len().to_le_bytes());
            for part in parts {
                key = fnv(key, &part.entity.to_le_bytes());
            }
        }
        if key != self.foliage_key {
            self.foliage_key = key;
            self.rebuild_foliage(view, &coords.iter().map(|c| (c.x, c.z)).collect(), gaps);
        }
        Ok(self.tiles.len())
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
                if self.foliage.len() == MAX_FOLIAGE_DRAWS {
                    over += 1;
                    continue;
                }
                let mut entity = fnv(FNV, &instance.identity[0].to_le_bytes());
                entity = fnv(entity, &instance.identity[1].to_le_bytes());
                entity = fnv(fnv(entity, &rule.to_le_bytes()), &(index as u32).to_le_bytes());
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
                &format!("{over} foliage parts beyond {MAX_FOLIAGE_DRAWS} are not drawn"),
            );
        }
    }
}

/// Uploads one tile: its mesh, its splat-tinted colour texture and a rough material.
fn place(engine: &mut Engine, tile: &TileMesh) -> Result<Tile> {
    let vertices: Vec<Vertex> = tile.source.vertices.iter().map(scene::vertex).collect();
    let mesh = engine.create_mesh(&MeshData {
        vertices: &vertices,
        indices: &tile.source.indices,
    })?;
    let texture = engine.create_texture(texture::chain(
        tile.texture.size,
        tile.texture.rgba.clone(),
        TextureRole::BaseColorSrgb,
    ))?;
    let material = engine.create_material(&MaterialDesc {
        base_color: [1.; 4],
        metallic: 0.,
        // As Ara's presenter draws its tile meshes.
        roughness: 0.95,
        double_sided: true,
        base_color_texture: Some(TextureRef {
            texture,
            // One texel per height sample across the tile: no wrap onto the far edge.
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
    engine.remove_texture(tile.texture)?;
    Ok(())
}
