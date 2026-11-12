//! Terrain and foliage on the Renderlab path: ordinary meshes.
//!
//! Each resident terrain tile is the world-space mesh `ara_runtime_render::terrain_mesh`
//! builds, at full resolution; tiles merge into chunks of `CHUNK` by `CHUNK` (one Renderlab
//! object, mesh, weights texture and material per chunk, ids per tile as ranges of its
//! triangles). Its surface is the splat
//! program (`splat`): the terrain's layer materials' colour, roughness and normal maps
//! blended by the tile's splat weights as Ara's renderer shades them, with side
//! projections and detail maps to the user's detail distance. Without the program each
//! layer's base colour factor is blended into one texture; a terrain without layer
//! materials has Ara's default white surface. Each foliage instance of a resident tile
//! draws the parts of its rule's model. What that leaves out (a gap-table row):
//! distance levels of detail, the virtual texture, and wind. Tiles and foliage follow
//! tile residency, so streaming still decides what exists.
use crate::rl::Engine;
use crate::{gaps::Gaps, scene, texture};
use anyhow::Result;
use ara_runtime::extract::Draw;
use ara_runtime_render::{
    ExternalView,
    terrain_mesh::{TerrainMeshes, TileMesh},
};
use glam34::{Mat4 as AraMat4, Quat, Vec3 as AraVec3};
use renderlab::engine::ProgramId;
use renderlab::{
    asset::{TangentBasis, TextureRole, Wrap},
    engine::{
        Mat4, MaterialDesc, MaterialId, MeshData, MeshId, ObjectId, Sampler, TangentStream,
        TextureId, TextureRef, Vertex,
    },
};
use std::collections::{HashMap, HashSet};

/// Foliage part draws kept at most: each is one Renderlab object.
const MAX_FOLIAGE_DRAWS: usize = 60_000;
/// Foliage triangles the non-RT path draws at most (`Terrain::sync`).
const NONRT_FOLIAGE_TRIANGLES: usize = 2_000_000;

/// Triangles a draw's mesh holds (shapes: a few hundred at most).
fn triangles_of(draw: &Draw) -> usize {
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

/// Tiles per chunk side: resident tiles merge into chunks of this many by this many.
const CHUNK: i32 = 4;

/// A resident tile of a chunk, as the merge reads it.
struct Member<'a> {
    coord: (i32, i32),
    mesh: std::sync::Arc<TileMesh>,
    /// One splat weight per height sample.
    weights: &'a [[u8; 4]],
    /// The tile's index among the frame's sorted resident coordinates (its editor id).
    index: u32,
}

/// The triangles of a chunk's mesh that one tile owns, and the tile's index.
struct TagRange {
    first_triangle: u32,
    triangles: u32,
    index: u32,
}

/// A chunk's merged geometry and weights, as `place` uploads them.
struct Merged {
    vertices: Vec<ara_runtime::model::ModelVertex>,
    indices: Vec<u32>,
    /// The weights texture's size: `CHUNK` blocks of one tile's samples per side.
    size: [u32; 2],
    weights: Vec<[u8; 4]>,
}

/// Merges the tiles of chunk `chunk` into one mesh and one weights texture. Each tile's
/// weights fill a block of the texture (blocks of absent tiles read the first layer), and its
/// texture coordinates move into that block: a tile's own sampling is kept (its mesh addressed
/// its texture's `side` texels over [0, 1]; the block is `side` texels of `CHUNK * side`).
fn merge(chunk: (i32, i32), members: &[Member<'_>]) -> Merged {
    let side = members
        .first()
        .map_or(1, |m| m.mesh.texture.size[0] as usize);
    let wide = CHUNK as usize * side;
    let mut weights = vec![[255, 0, 0, 0]; wide * wide];
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    let n = CHUNK as f32;
    for member in members {
        let (bx, bz) = (
            (member.coord.0 - chunk.0 * CHUNK) as usize,
            (member.coord.1 - chunk.1 * CHUNK) as usize,
        );
        if member.weights.len() == side * side {
            for z in 0..side {
                let row = (bz * side + z) * wide + bx * side;
                weights[row..row + side].copy_from_slice(&member.weights[z * side..(z + 1) * side]);
            }
        }
        let base = vertices.len() as u32;
        vertices.extend(member.mesh.source.vertices.iter().map(|v| {
            let mut v = *v;
            v.uv = [(bx as f32 + v.uv[0]) / n, (bz as f32 + v.uv[1]) / n];
            v
        }));
        indices.extend(member.mesh.source.indices.iter().map(|i| i + base));
    }
    Merged {
        vertices,
        indices,
        size: [wide as u32; 2],
        weights,
    }
}

/// Which triangles of the merged mesh each member tile owns, in `merge`'s order.
fn merge_ranges(members: &[Member<'_>]) -> Vec<TagRange> {
    let mut first = 0u32;
    members
        .iter()
        .map(|m| {
            let triangles = (m.mesh.source.indices.len() / 3) as u32;
            let range = TagRange {
                first_triangle: first,
                triangles,
                index: m.index,
            };
            first += triangles;
            range
        })
        .collect()
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
    /// The placed tiles' objects with their index among the frame's sorted resident tile
    /// coordinates, and the ids last set from them (`Terrain::tag`).
    tagged: Vec<(ObjectId, Vec<TagRange>)>,
    applied: Vec<(ObjectId, Vec<(u32, u32, u32)>)>,
    /// Foliage part draws of the resident tiles, for `Scene::sync`.
    foliage: Vec<Draw>,
    /// What `foliage` was built from; a new value means every draw may have changed.
    foliage_key: u64,
    /// The layer atlas and parameters, and the layers' key they were built for; `None`
    /// inside when the build failed (the tiles then show the layers' factors, and the
    /// build is not retried until the key changes).
    splat: Option<(u64, Option<crate::splat::Splat>)>,
    /// Decoded layer maps kept across rebuilds of the atlases.
    quadrants: crate::splat::Quadrants,
    /// The splat program once tried: `Some(None)` when Renderlab rejected it.
    splat_program: Option<Option<ProgramId>>,
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
        // Layer maps: one atlas per set of layers, rebuilt (and every tile re-placed,
        // as the tile keys carry the layers' key) when the layers change.
        let program = match layers.colours {
            Some(_) => crate::splat::program(&mut self.splat_program, engine, gaps),
            None => None,
        };
        let stale = self
            .splat
            .as_ref()
            .is_some_and(|(key, _)| *key != layers.key);
        let old_textures: Vec<TextureId> = if stale {
            self.splat
                .take()
                .and_then(|(_, s)| s)
                .map(|s| s.textures().collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if program.is_some() && self.splat.is_none() && layers.maps.iter().any(Option::is_some) {
            let built = crate::splat::build(
                engine,
                &layers.maps,
                view.terrain_detail,
                &mut self.quadrants,
                gaps,
            );
            // A build that fails (the device refused an atlas) costs the layers' maps,
            // not the frame.
            let built = built
                .map_err(|error| {
                    gaps.unsupported(
                        "terrain layer maps",
                        &format!("each layer shows its base colour factor ({error:#})"),
                    );
                })
                .ok();
            self.splat = Some((layers.key, built));
        }
        let splat = program.zip(self.splat.as_ref().and_then(|(_, s)| s.as_ref()));
        // Resident tiles merge into chunks of `CHUNK` by `CHUNK` tiles: one mesh, one weights
        // texture and one group each, as the tiles' per-collection cost in Renderlab's frame
        // is the same for a tile as for any object.
        let mut by_chunk: std::collections::BTreeMap<(i32, i32), Vec<Member>> = Default::default();
        for (index, coord) in coords.iter().enumerate() {
            let (Some(mesh), Some(tile)) = (
                self.meshes.tile(&terrain.data, *coord),
                terrain.data.tiles.get(coord),
            ) else {
                continue;
            };
            by_chunk
                .entry((coord.x.div_euclid(CHUNK), coord.z.div_euclid(CHUNK)))
                .or_default()
                .push(Member {
                    coord: (coord.x, coord.z),
                    mesh,
                    weights: &tile.weights,
                    index: index as u32,
                });
        }
        let wanted: Vec<_> = by_chunk
            .into_iter()
            .map(|(chunk, members)| {
                let key = members.iter().fold(
                    fnv(fnv(FNV, &chunk.0.to_le_bytes()), &chunk.1.to_le_bytes()),
                    |h, m| fnv(h, &(m.mesh.key ^ layers.key).to_le_bytes()),
                );
                (key, chunk, members)
            })
            .collect();
        if wanted.len() > budget {
            gaps.limit(
                "mesh-material groups per frame (terrain chunks)",
                &format!(
                    "{} of {} resident terrain chunks are not drawn",
                    wanted.len() - budget,
                    wanted.len()
                ),
            );
        }
        let keep: HashSet<u64> = wanted.iter().take(budget).map(|chunk| chunk.0).collect();
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
        for (key, chunk, members) in wanted.iter().take(budget) {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.tiles.entry(*key) {
                let merged = merge(*chunk, members);
                slot.insert(place(engine, &merged, &layers, splat)?);
            }
        }
        self.tagged = wanted
            .iter()
            .take(budget)
            .filter_map(|(key, _, members)| {
                Some((self.tiles.get(key)?.object, merge_ranges(members)))
            })
            .collect();
        for texture in old_textures {
            engine.remove_texture(texture)?;
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
                .map(|(_, _, members)| {
                    members
                        .iter()
                        .map(|m| m.mesh.source.indices.len() / 3)
                        .sum()
                })
                .chain(view.frame.draws.iter().map(triangles_of))
                .chain(view.foliage_parts.values().flatten().map(triangles_of))
                .fold(uploaded, usize::max)
        });
        let cap = if packed_ids.is_some() {
            packed_instances(largest)
                .saturating_sub(view.frame.draws.len() + budget)
                .min(MAX_FOLIAGE_DRAWS)
        } else {
            MAX_FOLIAGE_DRAWS
        };
        // Renderlab's non-RT shadows slow sharply with caster triangles (Outpost's ten
        // million foliage triangles outlive a GPU's hang timer): foliage stays within a
        // triangle budget there.
        let triangles = packed_ids.map(|_| NONRT_FOLIAGE_TRIANGLES);
        key = fnv(
            fnv(key, &cap.to_le_bytes()),
            &view.foliage_percent.to_le_bytes(),
        );
        key = fnv(key, &triangles.unwrap_or(0).to_le_bytes());
        if key != self.foliage_key {
            self.foliage_key = key;
            self.rebuild_foliage(
                view,
                &coords.iter().map(|c| (c.x, c.z)).collect(),
                cap,
                triangles,
                gaps,
            );
        }
        Ok((self.tiles.len(), largest))
    }

    /// Gives the placed chunks the editor's ids: tile `n` of the frame's sorted resident
    /// coordinates is `first + n` (the editor lists them after the frame's draws), one range of
    /// a chunk's triangles per member tile. Sets only what changed.
    pub fn tag(&mut self, engine: &mut Engine, first: u32) -> Result<()> {
        let wanted: Vec<(ObjectId, Vec<(u32, u32, u32)>)> = self
            .tagged
            .iter()
            .map(|(object, members)| {
                let ranges = members
                    .iter()
                    .map(|m| (m.first_triangle, m.triangles, first + m.index))
                    .collect();
                (*object, ranges)
            })
            .collect();
        if wanted != self.applied {
            for (object, ranges) in &wanted {
                engine.set_object_ids(*object, ranges)?;
            }
            self.applied = wanted;
        }
        Ok(())
    }

    /// The foliage draws and the key of what they were built from.
    pub fn foliage(&self) -> (&[Draw], u64) {
        (&self.foliage, self.foliage_key)
    }

    fn clear(&mut self, engine: &mut Engine) -> Result<()> {
        for (_, tile) in self.tiles.drain() {
            release(engine, tile)?;
        }
        self.tagged.clear();
        self.applied.clear();
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
        budget: Option<usize>,
        gaps: &mut Gaps,
    ) {
        self.foliage.clear();
        let Some(terrain) = view.frame.terrain.as_ref() else {
            return;
        };
        let data = &terrain.data;
        let (origin, width) = (data.header.origin, data.header.tile_width());
        let (mut unresolved, mut over) = (0usize, 0usize);
        let mut used = 0usize;
        for instance in &data.foliage {
            // The user's foliage density, selected as Ara's renderer selects it.
            let identity = u64::from(instance.identity[0]) | u64::from(instance.identity[1]) << 32;
            if identity % 100 >= u64::from(view.foliage_percent.min(100)) {
                continue;
            }
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
                let cost = triangles_of(part);
                if self.foliage.len() == cap || budget.is_some_and(|b| used + cost > b) {
                    over += 1;
                    continue;
                }
                let mut entity = fnv(FNV, &instance.identity[0].to_le_bytes());
                entity = fnv(entity, &instance.identity[1].to_le_bytes());
                entity = fnv(
                    fnv(entity, &rule.to_le_bytes()),
                    &(index as u32).to_le_bytes(),
                );
                used += cost;
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
                    "{over} foliage parts are not drawn: at most {cap} parts ({MAX_FOLIAGE_DRAWS} on the RT path; fewer on the non-RT path, whose shadows pack instance and triangle into 32 bits){}",
                    budget.map_or(String::new(), |b| format!(" and {b} triangles on the non-RT path, whose shadows slow sharply with caster triangles"))
                ),
            );
        }
    }
}

/// The terrain's splat layers as the bridge draws them, borrowed from the view.
struct Layers<'a> {
    /// Linear base colour factor per splat channel; `None` without layer materials.
    colours: Option<[[f32; 3]; 4]>,
    /// Each layer's material and cooked maps, for the splat atlases, one entry per layer
    /// in the splat's channel order; `None` for a layer whose material is not resolved
    /// (still streaming, or deleted), so the layers after it keep their channels.
    maps: Vec<Option<crate::splat::LayerMaps<'a>>>,
    roughness: f32,
    /// Changes when the colours do, so tiles are placed again.
    key: u64,
}

impl<'a> Layers<'a> {
    fn resolve(view: &ExternalView<'a>, gaps: &mut Gaps) -> Self {
        let references = view
            .frame
            .terrain
            .as_ref()
            .map_or(&[][..], |terrain| &terrain.data.header.materials[..]);
        if references.is_empty() {
            // As Ara's default surface: white, half rough.
            return Self {
                colours: None,
                maps: Vec::new(),
                roughness: 0.5,
                key: 0,
            };
        }
        if references.len() > 4 {
            gaps.unsupported(
                "terrain layers past the fourth",
                "the first four layers are drawn",
            );
        }
        let mut maps = Vec::new();
        let mut colours = [[1.; 3]; 4];
        let (mut roughness, mut key) = (0., FNV);
        for (colour, reference) in colours.iter_mut().zip(references) {
            let asset = view.terrain_materials.get(&reference.0);
            roughness +=
                asset.map_or(0.5, |a| a.material.roughness) / references.len().min(4) as f32;
            // One entry per layer, resolved or not, so splat channels keep their layers.
            maps.push(asset.map(|asset| {
                let material = &asset.material;
                colour.copy_from_slice(&material.base_color[..3]);
                key = fnv(key, &asset.digest.0);
                // A map's cache key: the material asset's digest and the texture slot.
                let map = |slot: Option<usize>| {
                    let i = slot?;
                    let texture = asset.textures.get(i)?;
                    Some((
                        fnv(fnv(FNV, &asset.digest.0), &i.to_le_bytes()),
                        &texture.ktx2[..],
                    ))
                };
                crate::splat::LayerMaps {
                    material,
                    color: map(material.base_color_texture),
                    normal: map(material.normal_texture),
                    metal_roughness: map(material.metallic_roughness_texture),
                    unsampled: material.occlusion_texture.is_some()
                        || material.emissive_texture.is_some(),
                }
            }));
            if asset.is_none() {
                key = fnv(key, b"unresolved");
            }
            for channel in colour {
                key = fnv(key, &channel.to_bits().to_le_bytes());
            }
        }
        // The detail distance shapes the splat's parameters: a new tier re-places tiles.
        key = fnv(key, &view.terrain_detail.to_bits().to_le_bytes());
        Self {
            colours: Some(colours),
            maps,
            roughness: roughness.clamp(0.045, 1.),
            key,
        }
    }
}

/// Tile tangents for the splat's frame: world +X projected onto each vertex normal,
/// with the handedness that puts the bitangent toward +Z on upward-facing ground.
fn splat_tangents(vertices: &[Vertex]) -> TangentStream {
    let values = vertices
        .iter()
        .map(|v| {
            let n = AraVec3::from_array(v.normal).normalize_or(AraVec3::Y);
            let x = AraVec3::X - n * n.x;
            let t = if x.length_squared() > 1e-8 {
                x.normalize()
            } else {
                (AraVec3::Z - n * n.z).normalize_or(AraVec3::Z)
            };
            let handedness = if n.cross(t).z < 0. { -1. } else { 1. };
            [t.x, t.y, t.z, handedness]
        })
        .collect();
    TangentStream {
        basis: TangentBasis::Authored,
        values,
    }
}

/// Uploads one tile: its mesh and a material coloured by its splat layers.
fn place(
    engine: &mut Engine,
    tile: &Merged,
    layers: &Layers<'_>,
    splat: Option<(ProgramId, &crate::splat::Splat)>,
) -> Result<Tile> {
    let weights = &tile.weights[..];
    let vertices: Vec<Vertex> = tile.vertices.iter().map(scene::vertex).collect();
    let data = MeshData {
        vertices: &vertices,
        indices: &tile.indices,
    };
    let fits = weights.len() as u32 == tile.size[0] * tile.size[1];
    // Layer normal maps need a tangent frame: the splat's (+X along the ground, the
    // bitangent toward +Z, as the layers' world X/Z coordinates run).
    let normals = splat.filter(|_| fits).and_then(|(_, s)| s.normals);
    let mesh = match normals {
        Some(_) => engine.create_mesh_with_tangents(&data, &splat_tangents(&vertices))?,
        None => engine.create_mesh(&data)?,
    };
    let clamp = Sampler {
        wrap_u: Wrap::Clamp,
        wrap_v: Wrap::Clamp,
        ..Default::default()
    };
    // Layer maps: the shared atlas, and this tile's weights (one texel per height
    // sample) in the metallic-roughness slot the splat program reads them from.
    if let Some((program, splat)) = splat.filter(|_| fits) {
        let rgba = weights.iter().flatten().copied().collect();
        let weights = engine.create_texture(texture::chain(
            tile.size,
            rgba,
            TextureRole::MetallicRoughnessLinear,
        ))?;
        let material = engine.create_material(&MaterialDesc {
            double_sided: true,
            base_color_texture: Some(TextureRef {
                texture: splat.atlas,
                sampler: clamp,
            }),
            metallic_roughness_texture: Some(TextureRef {
                texture: weights,
                sampler: clamp,
            }),
            // The program decodes and scales the layers' normals itself; a zero scale
            // keeps Renderlab's own normal-variance estimate off the atlas.
            normal_texture: normals.map(|texture| TextureRef {
                texture,
                sampler: clamp,
            }),
            normal_scale: 0.,
            program: Some(program),
            parameters: &splat.parameters,
            ..Default::default()
        })?;
        let object = engine.spawn(mesh, material, Mat4::IDENTITY)?;
        return Ok(Tile {
            object,
            mesh,
            material,
            texture: Some(weights),
        });
    }
    // One texel per height sample, as the tile mesh's coordinates address them.
    let texture = match layers.colours {
        Some(colours) if weights.len() as u32 == tile.size[0] * tile.size[1] => {
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
                tile.size,
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
    /// A tile of `side` by `side` samples whose two triangles and weights are marked by `mark`.
    fn tile(mark: u8, side: u32) -> (std::sync::Arc<TileMesh>, Vec<[u8; 4]>) {
        use ara_runtime::model::ModelVertex;
        let vertex = |u: f32, v: f32| ModelVertex {
            uv: [u, v],
            ..Default::default()
        };
        let mesh = TileMesh {
            entity: 0,
            label: String::new(),
            key: u64::from(mark),
            source: std::sync::Arc::new(ara_runtime_render::geometry::Source {
                vertices: vec![
                    vertex(0., 0.),
                    vertex(1., 0.),
                    vertex(0., 1.),
                    vertex(1., 1.),
                ],
                indices: vec![0, 2, 1, 1, 2, 3],
            }),
            texture: std::sync::Arc::new(ara_frame::scene::Texture {
                size: [side, side],
                rgba: Vec::new(),
                srgb: true,
            }),
        };
        (
            std::sync::Arc::new(mesh),
            vec![[mark, 0, 0, 0]; (side * side) as usize],
        )
    }

    /// Each tile's weights land in its block, its texture coordinates move into that block,
    /// and its triangles are the range its id is set on.
    #[test]
    fn a_chunk_keeps_each_tiles_block_coordinates_and_triangles() {
        let side = 3;
        let ((a, wa), (b, wb)) = (tile(7, side), tile(9, side));
        let members = [
            Member {
                coord: (4, 4),
                mesh: a,
                weights: &wa,
                index: 10,
            },
            Member {
                coord: (5, 4),
                mesh: b,
                weights: &wb,
                index: 11,
            },
        ];
        let merged = merge((1, 1), &members);
        let wide = 4 * side as usize;
        assert_eq!(merged.size, [wide as u32; 2]);
        // Block (0, 0) holds the first tile's weights and block (1, 0) the second's;
        // everything else reads the first layer.
        assert_eq!(merged.weights[0], [7, 0, 0, 0]);
        assert_eq!(merged.weights[side as usize], [9, 0, 0, 0]);
        assert_eq!(merged.weights[2 * side as usize], [255, 0, 0, 0]);
        // The second tile's far corner is at the end of its block of four.
        assert_eq!(merged.vertices.len(), 8);
        assert_eq!(merged.vertices[7].uv, [2. / 4., 1. / 4.]);
        assert_eq!(merged.indices[6..9], [4, 6, 5]);
        let ranges = merge_ranges(&members);
        assert_eq!(
            ranges
                .iter()
                .map(|r| (r.first_triangle, r.triangles, r.index))
                .collect::<Vec<_>>(),
            [(0, 2, 10), (2, 2, 11)]
        );
    }
}
