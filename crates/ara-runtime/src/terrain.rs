//! Terrain in play. A [`Terrain`] component names a `.terrain` document (and a
//! `.foliage` scatter over it): every tile evaluates on worker threads at build,
//! navigation bakes over the heightfields, foliage scatters once for the renderer and
//! [`TerrainState`] keeps the immutable tiles.
//!
//! Collision follows [`TerrainTile`] entities (authored in world cells, so it streams
//! with them; or in the level): each becomes an ECS-bound static Jolt heightfield
//! body (`Collider { kind: Cooked }`), so snapshots, restores, replays and network
//! replication treat it like any body. Cooked shapes are runtime handles: they are
//! re-cooked after every native rebuild ([`cook_tiles`]).
use crate::{Result, RuntimeError, assets::asset_id};
use ara_doc::Workspace;
use ara_foliage::{Bounds, Instance, Rule, SurfaceSample};
use ara_physics::{
    Collider, ColliderKind, CookedCollider, MotionType, PhysicsId, PhysicsPose, PhysicsWorld,
    RigidBody,
};
use ara_physics_jolt::JoltBackend;
use ara_schema::{AssetRef, Schema};
use ara_terrain::{Header, Tile, TileCoord, cache::TileSource};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

/// Puts a `.terrain` heightfield (and its `.foliage` scatter) in play: rendered by the
/// landscape renderer, walkable for navigation, solid where `TerrainTile`s exist.
#[derive(Debug, Clone, Default, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "terrain")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Terrain {
    /// The heightfield manifest (`.terrain` document).
    #[ara(asset_kind = "terrain")]
    pub terrain: Option<AssetRef>,
    /// Scatter rules over it (`.foliage` document), drawn by the landscape renderer.
    #[ara(asset_kind = "foliage")]
    pub foliage: Option<AssetRef>,
}

/// Collision for one terrain tile (grid coordinates of the level's `Terrain`). Author
/// these in world cells to stream collision with the cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "terrain")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TerrainTile {
    /// Tile index along world X.
    pub x: i32,
    /// Tile index along world Z.
    pub z: i32,
}

/// The evaluated terrain of a world: grid, immutable tiles and foliage instances.
#[derive(Resource, Clone)]
pub struct TerrainState(pub Arc<TerrainData>);

/// Immutable terrain content shared with presentation.
#[derive(Debug)]
pub struct TerrainData {
    /// Grid settings.
    pub header: Header,
    /// Every tile, evaluated at build.
    pub tiles: BTreeMap<TileCoord, Tile>,
    /// Foliage instances scattered over every tile (renderer input).
    pub foliage: Vec<Instance>,
}

impl TerrainData {
    /// Height, normal and splat weights at world X/Z (bilinear), if a tile covers it.
    pub fn surface(&self, p: [f32; 2]) -> Option<SurfaceSample> {
        let h = |x: f32, z: f32| self.height(x, z);
        let center = h(p[0], p[1])?;
        let s = self.header.spacing;
        let dx = (h(p[0] + s, p[1]).unwrap_or(center) - h(p[0] - s, p[1]).unwrap_or(center))
            / (2.0 * s);
        let dz = (h(p[0], p[1] + s).unwrap_or(center) - h(p[0], p[1] - s).unwrap_or(center))
            / (2.0 * s);
        let n = glam::Vec3::new(-dx, 1.0, -dz).normalize();
        let (tile, i) = self.sample_index(p[0], p[1])?;
        let w = tile.weights.get(i).copied().unwrap_or([255, 0, 0, 0]);
        Some(SurfaceSample {
            height: center,
            normal: n.to_array(),
            weights: w.map(|c| f32::from(c) / 255.0),
        })
    }

    /// Bilinear height at world X/Z, if a tile covers it.
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        let header = &self.header;
        let width = header.tile_width();
        let (lx, lz) = (x - header.origin[0], z - header.origin[1]);
        let coord = TileCoord {
            x: (lx / width).floor() as i32,
            z: (lz / width).floor() as i32,
        };
        let tile = self.tiles.get(&coord)?;
        let n = header.cells as usize + 1;
        let fx = (lx - coord.x as f32 * width) / header.spacing;
        let fz = (lz - coord.z as f32 * width) / header.spacing;
        let (ix, iz) = (fx.floor().max(0.0) as usize, fz.floor().max(0.0) as usize);
        let (ix, iz) = (ix.min(n - 2), iz.min(n - 2));
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let at = |x: usize, z: usize| tile.heights[z * n + x];
        let top = at(ix, iz) * (1.0 - tx) + at(ix + 1, iz) * tx;
        let bottom = at(ix, iz + 1) * (1.0 - tx) + at(ix + 1, iz + 1) * tx;
        Some(top * (1.0 - tz) + bottom * tz)
    }

    fn sample_index(&self, x: f32, z: f32) -> Option<(&Tile, usize)> {
        let header = &self.header;
        let width = header.tile_width();
        let (lx, lz) = (x - header.origin[0], z - header.origin[1]);
        let coord = TileCoord {
            x: (lx / width).floor() as i32,
            z: (lz / width).floor() as i32,
        };
        let tile = self.tiles.get(&coord)?;
        let n = header.cells as usize + 1;
        let ix = (((lx - coord.x as f32 * width) / header.spacing).round() as usize).min(n - 1);
        let iz = (((lz - coord.z as f32 * width) / header.spacing).round() as usize).min(n - 1);
        Some((tile, iz * n + ix))
    }
}

/// Base tiles are not imported in play yet: terrains are built from strokes.
struct NoBases;

impl TileSource for NoBases {
    fn load(&self, hash: &str) -> std::result::Result<Tile, ara_diag::Diagnostic> {
        Err(ara_terrain::invalid(format!(
            "imported terrain base {hash} is not available in play"
        )))
    }
}

/// Evaluates the level's terrain (the first `Terrain` component) on worker threads and
/// scatters its foliage. `None` without a terrain.
pub fn build(world: &mut World, workspace: &Workspace) -> Result<Option<TerrainState>> {
    let mut query = world.query::<&Terrain>();
    let Some(component) = query.iter(world).find(|t| t.terrain.is_some()).cloned() else {
        return Ok(None);
    };
    let document = |reference: &AssetRef, kind: &str| {
        let id = asset_id(reference)?;
        workspace
            .doc(id)
            .filter(|d| d.kind == kind)
            .ok_or_else(|| RuntimeError(format!("{} is not a loaded {kind} document", reference.0)))
    };
    let reference = component
        .terrain
        .as_ref()
        .ok_or_else(|| RuntimeError("Terrain without a terrain document".into()))?;
    let terrain = ara_terrain::Terrain::from_document(document(reference, "terrain")?)
        .map_err(|d| RuntimeError(format!("{}: {}", reference.0, d.message)))?;
    let header = terrain.header.clone();
    let coords: Vec<TileCoord> = (header.min.z..header.max.z)
        .flat_map(|z| (header.min.x..header.max.x).map(move |x| TileCoord { x, z }))
        .collect();
    // Worker threads: evaluation (strokes, erosion halos) never runs on a tick.
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
    let chunks: Vec<&[TileCoord]> = coords.chunks(coords.len().div_ceil(workers).max(1)).collect();
    let evaluated: Vec<std::result::Result<Vec<(TileCoord, Tile)>, String>> =
        std::thread::scope(|scope| {
            let handles: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    let terrain = &terrain;
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|&c| {
                                terrain
                                    .evaluate(c, &NoBases)
                                    .map(|t| (c, t))
                                    .map_err(|d| d.message)
                            })
                            .collect()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err("terrain worker panicked".into())))
                .collect()
        });
    let mut tiles = BTreeMap::new();
    for chunk in evaluated {
        tiles.extend(chunk.map_err(|e| RuntimeError(format!("terrain: {e}")))?);
    }
    let mut data = TerrainData {
        header,
        tiles,
        foliage: Vec::new(),
    };
    if let Some(reference) = &component.foliage {
        data.foliage = scatter(&data, document(reference, "foliage")?)?;
    }
    Ok(Some(TerrainState(Arc::new(data))))
}

/// Every enabled foliage rule over every tile (renderer input, deterministic).
fn scatter(data: &TerrainData, doc: &ara_doc::Document) -> Result<Vec<Instance>> {
    let mut rules: Vec<(String, Rule)> = Vec::new();
    for (id, record) in doc.records() {
        if !id.as_str().starts_with("r_") {
            continue;
        }
        let rule: Rule = serde_json::from_value(serde_json::Value::Object(record.clone()))?;
        if rule.enabled {
            rules.push((id.to_string(), rule));
        }
    }
    let width = data.header.tile_width();
    let surface = |p: [f32; 2]| data.surface(p);
    let mut instances = Vec::new();
    for coord in data.tiles.keys() {
        let min = [
            data.header.origin[0] + coord.x as f32 * width,
            data.header.origin[1] + coord.z as f32 * width,
        ];
        let bounds = Bounds {
            min,
            max: [min[0] + width, min[1] + width],
        };
        for (index, (_, rule)) in rules.iter().enumerate() {
            instances.extend(
                ara_foliage::scatter(index as u64, rule, bounds, &surface, &[])
                    .map_err(|d| RuntimeError(format!("foliage: {}", d.message)))?,
            );
        }
    }
    Ok(instances)
}

/// Owns the build-time terrain resource (not gameplay state: immutable content).
pub struct TerrainProvider;

impl ara_sim::StateProvider for TerrainProvider {
    fn name(&self) -> &'static str {
        "terrain"
    }
    fn owned_types(&self) -> Vec<std::any::TypeId> {
        vec![std::any::TypeId::of::<TerrainState>()]
    }
    fn capture(&self, _: &World) -> std::result::Result<serde_json::Value, ara_sim::SimError> {
        Ok(serde_json::Value::Null)
    }
    fn restore(
        &self,
        _: &mut World,
        _: &serde_json::Value,
    ) -> std::result::Result<(), ara_sim::SimError> {
        Ok(())
    }
}

/// Stable physics identity of a terrain tile body (high bits mark terrain).
pub fn tile_physics_id(coord: TileCoord) -> u64 {
    0x7e44_0000_0000_0000 | (u64::from(coord.x as u16) << 16) | u64::from(coord.z as u16)
}

/// Gives every `TerrainTile` without a cooked collider its static heightfield body:
/// tiles spawned by streaming, and all of them after a native physics rebuild.
pub fn cook_tiles(world: &mut World) -> Result<()> {
    let Some(state) = world.get_resource::<TerrainState>().cloned() else {
        return Ok(());
    };
    let mut wanted = Vec::new();
    let mut query = world.query_filtered::<(Entity, &TerrainTile), Without<CookedCollider>>();
    for (entity, tile) in query.iter(world) {
        wanted.push((entity, TileCoord { x: tile.x, z: tile.z }));
    }
    if wanted.is_empty() {
        return Ok(());
    }
    wanted.sort_by_key(|(_, c)| *c);
    let header = &state.0.header;
    for (entity, coord) in wanted {
        let Some(tile) = state.0.tiles.get(&coord) else {
            continue;
        };
        let shape = {
            let mut physics = world.resource_mut::<PhysicsWorld<JoltBackend>>();
            tile.cook_collider(header, coord, physics.backend_mut())?
        };
        let Some(shape) = shape else {
            continue;
        };
        world.entity_mut(entity).insert((
            RigidBody {
                motion: MotionType::Static,
                ..Default::default()
            },
            Collider {
                kind: ColliderKind::Cooked,
                friction: 0.8,
                ..Default::default()
            },
            CookedCollider(shape),
            PhysicsId {
                value: tile_physics_id(coord),
            },
            PhysicsPose::default(),
        ));
    }
    Ok(())
}

/// The fixed-tick hook: streamed-in tiles get their bodies before physics.
pub fn adopt_tiles(world: &mut World) {
    if let Err(error) = cook_tiles(world) {
        world.resource_mut::<ara_physics::PhysicsStatus>().error =
            Some(ara_physics::PhysicsError(format!("terrain: {}", error.0)));
    }
}

/// Every tile's heightfield as navigation geometry (area 0).
pub fn navigation_geometry(state: &TerrainState, geometry: &mut ara_nav::Geometry) -> Result<()> {
    for (coord, tile) in &state.0.tiles {
        let field = tile.heightfield(&state.0.header, *coord);
        geometry
            .append_heightfield(
                field.origin,
                field.spacing,
                field.cells,
                field.heights,
                field.holes,
                0,
            )
            .map_err(|e| RuntimeError(format!("terrain navigation: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_physics_ids_are_unique_per_coordinate() {
        let a = tile_physics_id(TileCoord { x: 1, z: 2 });
        let b = tile_physics_id(TileCoord { x: 2, z: 1 });
        let c = tile_physics_id(TileCoord { x: -1, z: 2 });
        assert!(a != b && a != c && b != c);
    }
}
