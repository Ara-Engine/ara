//! Terrain in play. A [`Terrain`] component names a `.terrain` document (and a
//! `.foliage` scatter over it): every tile evaluates on worker threads at build,
//! navigation bakes over the heightfields, foliage scatters once for the renderer and
//! [`TerrainState`] keeps the immutable tiles.
//!
//! Collision follows [`TerrainChunk`] entities (authored in world cells, so it streams
//! with them; or in the level): each becomes an ECS-bound static Jolt heightfield
//! body (`Collider { kind: Cooked }`), so snapshots, restores, replays and network
//! replication treat it like any body. Cooked shapes are runtime handles: they are
//! re-cooked after every native rebuild ([`cook_chunks`]).
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
/// landscape renderer, walkable for navigation, solid where `TerrainChunk`s exist.
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
pub struct TerrainChunk {
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
    /// Every source rule, including disabled rules, in stable qualified-ID order.
    /// `Instance.identity[3]` indexes this generation's catalog.
    pub foliage_rules: Vec<FoliageRule>,
}

/// Immutable rule identity and authoring data for resolving model/prefab geometry.
#[derive(Debug, Clone)]
pub struct FoliageRule {
    /// Stable document and record identity, for example `fo_meadow#r_shrub`.
    /// Moving the document or enabling another rule does not change its scatter seed.
    pub id: String,
    /// Source authoring rule. Disabled rules retain their catalog slot.
    pub rule: Rule,
}

impl TerrainData {
    /// Height, normal and splat weights at world X/Z (bilinear), if a tile covers it.
    pub fn surface(&self, p: [f32; 2]) -> Option<SurfaceSample> {
        let h = |x: f32, z: f32| self.height(x, z);
        let center = h(p[0], p[1])?;
        let s = self.header.spacing;
        let dx =
            (h(p[0] + s, p[1]).unwrap_or(center) - h(p[0] - s, p[1]).unwrap_or(center)) / (2.0 * s);
        let dz =
            (h(p[0], p[1] + s).unwrap_or(center) - h(p[0], p[1] - s).unwrap_or(center)) / (2.0 * s);
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

/// Evaluates and scatters in authored terrain space, then applies the scene entity's
/// world translation to heights, grid origin and foliage. `None` without a terrain.
pub fn build(world: &mut World, workspace: &Workspace) -> Result<Option<TerrainState>> {
    let mut query = world.query::<(Entity, &Terrain)>();
    let Some((entity, component)) = query
        .iter(world)
        .find(|(_, t)| t.terrain.is_some())
        .map(|(e, t)| (e, t.clone()))
    else {
        return Ok(None);
    };
    let matrix = crate::sync::world_matrices(world)
        .get(&entity)
        .copied()
        .unwrap_or(glam::Mat4::IDENTITY);
    let offset = matrix.w_axis.truncate();
    if !matrix.is_finite() || !matrix.abs_diff_eq(glam::Mat4::from_translation(offset), 1e-5) {
        return Err(RuntimeError(
            "Terrain scene transforms support translation; bake rotation and scale into its grid"
                .into(),
        ));
    }
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
    let chunks: Vec<&[TileCoord]> = coords
        .chunks(coords.len().div_ceil(workers).max(1))
        .collect();
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
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err("terrain worker panicked".into()))
                })
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
        foliage_rules: Vec::new(),
    };
    if let Some(reference) = &component.foliage {
        let (rules, instances) = scatter(&data, document(reference, "foliage")?)?;
        data.foliage_rules = rules;
        data.foliage = instances;
    }
    translate(&mut data, offset)?;
    Ok(Some(TerrainState(Arc::new(data))))
}

fn translate(data: &mut TerrainData, offset: glam::Vec3) -> Result<()> {
    data.header.origin[0] += offset.x;
    data.header.origin[1] += offset.z;
    data.header
        .validate()
        .map_err(|d| RuntimeError(d.message))?;
    for tile in data.tiles.values_mut() {
        for height in &mut tile.heights {
            *height += offset.y;
        }
        tile.validate().map_err(|d| RuntimeError(d.message))?;
    }
    for instance in &mut data.foliage {
        for (lane, delta) in instance.position_scale[..3]
            .iter_mut()
            .zip(offset.to_array())
        {
            *lane += delta;
        }
    }
    Ok(())
}

/// Every enabled foliage rule over every tile, minus the document's exclusion zones
/// (renderer input, deterministic).
fn scatter(
    data: &TerrainData,
    doc: &ara_doc::Document,
) -> Result<(Vec<FoliageRule>, Vec<Instance>)> {
    let mut rules = Vec::new();
    let mut exclusions: Vec<ara_foliage::Exclusion> = Vec::new();
    for (id, record) in doc.records() {
        let value = || serde_json::Value::Object(record.clone());
        if id.as_str().starts_with("x_") {
            exclusions.push(serde_json::from_value(value())?);
            continue;
        }
        if !id.as_str().starts_with("r_") {
            continue;
        }
        rules.push(FoliageRule {
            id: format!("{}#{id}", doc.id),
            rule: serde_json::from_value(value())?,
        });
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
        for (index, source) in rules
            .iter()
            .enumerate()
            .filter(|(_, source)| source.rule.enabled)
        {
            let index =
                u32::try_from(index).map_err(|_| RuntimeError("too many foliage rules".into()))?;
            let seed = crate::math::stable_id(&source.id);
            let mut placed =
                ara_foliage::scatter(seed, &source.rule, bounds, &surface, &exclusions)
                    .map_err(|d| RuntimeError(format!("foliage: {}", d.message)))?;
            for instance in &mut placed {
                instance.identity[3] = index;
            }
            instances.extend(placed);
        }
    }
    Ok((rules, instances))
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

/// Stable render identity of the terrain tile at `x`, `z` (a `TerrainChunk`'s
/// coordinates): every resident tile draws with it, so an editor maps a tile's pixels
/// back to its chunk entity (selection, hover and picking treat the terrain as one
/// object instead of loose tiles).
pub fn tile_render_id(x: i32, z: i32) -> u64 {
    crate::math::stable_id(&format!("terrain-tile:{x},{z}"))
}

/// Gives every `TerrainChunk` without a cooked collider its static heightfield body:
/// tiles spawned by streaming, and all of them after a native physics rebuild.
pub fn cook_chunks(world: &mut World) -> Result<()> {
    let Some(state) = world.get_resource::<TerrainState>().cloned() else {
        return Ok(());
    };
    let mut wanted = Vec::new();
    let mut query = world.query_filtered::<(Entity, &TerrainChunk), Without<CookedCollider>>();
    for (entity, tile) in query.iter(world) {
        wanted.push((
            entity,
            TileCoord {
                x: tile.x,
                z: tile.z,
            },
        ));
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
        // The immutable heightfield already contains the complete world offset.
        // Mark the chunk's local transform synchronized so its parent does not
        // teleport that baked geometry a second time on the first physics push.
        if let Some(transform) = world.get::<ara_components::Transform>(entity).copied() {
            world
                .entity_mut(entity)
                .insert(crate::sync::SyncedTransform(transform));
        }
    }
    Ok(())
}

/// The fixed-tick hook: streamed-in tiles get their bodies before physics.
pub fn adopt_chunks(world: &mut World) {
    if let Err(error) = cook_chunks(world) {
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
    fn foliage_rule_catalog_keeps_sources_without_reseeding_surviving_rules() {
        use ara_doc::{AssetId, Document, RecordId};
        let header = Header {
            origin: [0.; 2],
            spacing: 1.,
            cells: 32,
            min: TileCoord { x: 0, z: 0 },
            max: TileCoord { x: 1, z: 1 },
            materials: Vec::new(),
        };
        let terrain =
            ara_terrain::Terrain::new(AssetId::parse("tr_flat").expect("ID"), header.clone())
                .expect("terrain");
        let coordinate = TileCoord { x: 0, z: 0 };
        let data = TerrainData {
            header,
            tiles: BTreeMap::from([(
                coordinate,
                terrain.evaluate(coordinate, &NoBases).expect("tile"),
            )]),
            foliage: Vec::new(),
            foliage_rules: Vec::new(),
        };
        let record = serde_json::json!({
            "name":"Shrub", "asset":"@shrub.prefab#pf_shrub", "species":"grass",
            "seed":9, "cellSize":4., "density":0.05, "height":[-1.,1.],
            "slope":[0.,90.], "scale":[1.,1.], "enabled":true
        })
        .as_object()
        .expect("record")
        .clone();
        let mut document = Document::new("foliage", AssetId::parse("fo_meadow").expect("ID"));
        document.insert_unchecked(RecordId::parse("r_z").expect("ID"), record.clone());
        let (catalog, original) = scatter(&data, &document).expect("scatter");
        assert_eq!(catalog[0].id, "fo_meadow#r_z");
        assert_eq!(catalog[0].rule.asset.0, "@shrub.prefab#pf_shrub");
        assert!(!original.is_empty());
        assert!(original.iter().all(|instance| instance.identity[3] == 0));

        let mut disabled = record.clone();
        disabled.insert("enabled".into(), false.into());
        document.insert_unchecked(RecordId::parse("r_a").expect("ID"), disabled);
        let (catalog, mut after) = scatter(&data, &document).expect("disabled rule");
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].id, "fo_meadow#r_a");
        assert!(!catalog[0].rule.enabled);
        assert!(after.iter().all(|instance| instance.identity[3] == 1));
        for instance in &mut after {
            instance.identity[3] = 0;
        }
        assert_eq!(
            original, after,
            "inserting a disabled rule must preserve scatter identity and placement"
        );

        document.insert_unchecked(RecordId::parse("r_a").expect("ID"), record.clone());
        let (_, after) = scatter(&data, &document).expect("enabled rule");
        let survivors: Vec<_> = after
            .into_iter()
            .filter(|instance| instance.identity[3] == 1)
            .map(|mut instance| {
                instance.identity[3] = 0;
                instance
            })
            .collect();
        assert_eq!(
            original, survivors,
            "enabling another rule must preserve survivors"
        );

        let mut other = Document::new("foliage", AssetId::parse("fo_other").expect("ID"));
        other.insert_unchecked(RecordId::parse("r_z").expect("ID"), record);
        let (_, other) = scatter(&data, &other).expect("other document");
        assert_ne!(
            original, other,
            "document identity namespaces rule randomness"
        );
    }

    #[test]
    fn parented_terrain_translation_reaches_heights_foliage_navigation_and_collision() {
        use ara_components::Transform;
        use ara_doc::{AssetId, Document, RecordId};
        use ara_physics::{PhysicsConfig, Query, QueryFilter};

        let header = Header {
            origin: [125., -300.],
            spacing: 2.,
            cells: 32,
            min: TileCoord { x: -2, z: 3 },
            max: TileCoord { x: -1, z: 4 },
            materials: Vec::new(),
        };
        let terrain = ara_terrain::Terrain::new(AssetId::parse("tr_ground").expect("ID"), header)
            .expect("terrain")
            .to_document()
            .expect("document");
        let mut foliage = Document::new("foliage", AssetId::parse("fo_ground").expect("ID"));
        let rule = Rule {
            name: "Grass".into(),
            species: ara_foliage::Species::Grass,
            asset: AssetRef("@grass.glb#as_grass".into()),
            seed: 7,
            cell_size: 4.,
            density: 0.05,
            height: [-10., 10.],
            slope: [0., 90.],
            channel: 0,
            min_weight: 0.,
            noise: 0.,
            scale: [1., 1.],
            enabled: true,
        };
        let value = serde_json::to_value(rule)
            .expect("rule")
            .as_object()
            .expect("object")
            .clone();
        foliage.insert_unchecked(RecordId::parse("r_grass").expect("ID"), value);
        let mut workspace = Workspace::default();
        workspace.insert("ground.terrain", terrain);
        workspace.insert("ground.foliage", foliage);
        let mut world = World::new();
        let parent = world
            .spawn(Transform {
                position: [10., 7., -20.],
                ..Default::default()
            })
            .id();
        let terrain_entity = world
            .spawn((
                Terrain {
                    terrain: Some(AssetRef("@ground.terrain#tr_ground".into())),
                    foliage: Some(AssetRef("@ground.foliage#fo_ground".into())),
                },
                Transform::default(),
                ChildOf(parent),
            ))
            .id();
        let chunk = world
            .spawn((
                TerrainChunk { x: -2, z: 3 },
                Transform::default(),
                ChildOf(terrain_entity),
            ))
            .id();
        let state = build(&mut world, &workspace)
            .expect("translated build")
            .expect("terrain");
        assert_eq!(state.0.header.origin, [135., -320.]);
        assert_eq!(state.0.height(20., -110.), Some(7.));
        assert!(!state.0.foliage.is_empty());
        assert!(state.0.foliage.iter().all(|i| i.position_scale[1] == 7.
            && (7. ..71.).contains(&i.position_scale[0])
            && (-128. ..-64.).contains(&i.position_scale[2])));
        let mut nav = ara_nav::Geometry::default();
        navigation_geometry(&state, &mut nav).expect("navigation geometry");
        assert!(!nav.vertices.is_empty());
        assert!(
            nav.vertices.iter().all(|p| p[1] == 7.
                && (7. ..=71.).contains(&p[0])
                && (-128. ..=-64.).contains(&p[2]))
        );
        world.insert_resource(state);
        crate::physics::install(&mut world, &PhysicsConfig::default(), &[]).expect("physics");
        crate::sync::resolve_poses(&mut world);
        let mut schedule = Schedule::default();
        schedule.add_systems(crate::sync::push_poses);
        schedule.run(&mut world);
        assert_eq!(
            *world.get::<PhysicsPose>(chunk).expect("pose"),
            PhysicsPose::default()
        );
        ara_physics::tick_ecs::<JoltBackend>(&mut world).expect("physics tick");
        let hit = world
            .resource::<PhysicsWorld<JoltBackend>>()
            .query_first(Query::Ray {
                origin: [20., 30., -110.],
                displacement: [0., -50., 0.],
                filter: QueryFilter::default(),
            })
            .expect("ray")
            .expect("translated terrain collision");
        assert!((hit.point[1] - 7.).abs() < 0.01, "{hit:?}");
        assert_eq!(hit.user_data, tile_physics_id(TileCoord { x: -2, z: 3 }));
    }

    #[test]
    fn tile_physics_ids_are_unique_per_coordinate() {
        let a = tile_physics_id(TileCoord { x: 1, z: 2 });
        let b = tile_physics_id(TileCoord { x: 2, z: 1 });
        let c = tile_physics_id(TileCoord { x: -1, z: 2 });
        assert!(a != b && a != c && b != c);
    }
}
