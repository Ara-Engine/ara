//! Per-frame instance building: stable painter sort, chunked tile batches, compact IDs.
//! Runs on every `Renderer2d::update`, so it borrows from the extraction and allocates
//! only the output vectors.
use crate::Scene2d;
use anyhow::{Result, ensure};
use ara_2d::tilemap;
use bytemuck::{Pod, Zeroable};
use std::collections::BTreeMap;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Instance {
    pose: [f32; 4],
    size_pivot: [f32; 4],
    uv: [f32; 4],
    borders: [f32; 4],
    color: [f32; 4],
    info: [u32; 4],
    parallax_source: [f32; 4],
    basis: [f32; 4],
}
pub(crate) struct Batch {
    pub first: u32,
    pub count: u32,
    pub vertices: u32,
    pub bounds: Option<Bounds>,
}
pub(crate) struct Prepared {
    pub instances: Vec<Instance>,
    pub batches: Vec<Batch>,
    /// Compact render ID - 1 to stable identity and name; None when unchanged.
    pub names: Option<Vec<(u64, String)>>,
}
/// Painter key: sorting layer, order, entity, layer rank (0 = sprite), chunk. Unique by
/// construction once entity identities are unique.
type Key = (i32, i32, u64, u32, [i32; 2]);
/// A chunk's world-space culling rectangle and its parallax factor.
type Bounds = ([f32; 4], [f32; 2]);
/// One borrowed draw; tiles never allocate a Sprite.
struct Draw<'a> {
    position: [f32; 2],
    basis: [f32; 4],
    atlas: &'a str,
    normal: Option<&'a str>,
    region: &'a str,
    size: [f32; 2],
    tint: [f32; 4],
    parallax: [f32; 2],
    flags: u32,
}

pub(crate) fn prepare(
    scene: &Scene2d,
    indices: &BTreeMap<String, u32>,
    previous: &[(u64, String)],
) -> Result<Prepared> {
    ensure!(
        scene.sprites.len() <= 1_000_000,
        "sprite extraction exceeds capacity"
    );
    let mut identities: Vec<(u64, &str)> = scene
        .sprites
        .iter()
        .map(|s| (s.entity, s.name.as_str()))
        .chain(scene.tilemaps.iter().map(|m| (m.entity, m.name.as_str())))
        .collect();
    identities.sort_unstable_by_key(|(entity, _)| *entity);
    ensure!(
        identities.windows(2).all(|p| p[0].0 != p[1].0),
        "duplicate entity identity"
    );
    let id = |entity: u64| {
        identities
            .binary_search_by_key(&entity, |(e, _)| *e)
            .map(|i| i as u32 + 1)
            .map_err(|_| anyhow::anyhow!("unknown entity"))
    };
    let unchanged = identities.len() == previous.len()
        && identities
            .iter()
            .zip(previous)
            .all(|(a, b)| a.0 == b.0 && a.1 == b.1);
    let names = (!unchanged).then(|| {
        identities
            .iter()
            .map(|(e, n)| (*e, (*n).to_owned()))
            .collect()
    });
    let mut resolver = Resolver::new(scene, indices);
    let mut sprites: Vec<(Key, u32, Instance)> = Vec::with_capacity(scene.sprites.len());
    for sprite in &scene.sprites {
        let s = &sprite.sprite;
        let draw = Draw {
            position: sprite.position,
            basis: sprite.basis,
            atlas: &s
                .atlas
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("sprite {} has no atlas", sprite.name))?
                .0,
            normal: s.normal_atlas.as_ref().map(|n| n.0.as_str()),
            region: &s.region,
            size: s.size,
            tint: s.tint,
            parallax: s.parallax,
            flags: u32::from(s.nine_slice)
                | (u32::from(s.lit) << 1)
                | (u32::from(s.flip_x) << 2)
                | (u32::from(s.flip_y) << 3),
        };
        let key = (s.sorting_layer, s.order, sprite.entity, 0, [0; 2]);
        let vertices = if s.nine_slice { 54 } else { 6 };
        sprites.push((key, vertices, resolver.instance(&draw, id(sprite.entity)?)?));
    }
    sprites.sort_unstable_by_key(|(key, ..)| *key);
    // (key, instances, world bounds, parallax) per tile chunk.
    let mut chunks: Vec<(Key, Vec<Instance>, Bounds)> = Vec::new();
    for map in &scene.tilemaps {
        map.map.validate()?;
        let atlas = map
            .map
            .atlas
            .as_ref()
            .map(|a| a.0.as_str())
            .ok_or_else(|| anyhow::anyhow!("tilemap {} has no atlas", map.name))?;
        let map_id = id(map.entity)?;
        // Tiles larger than their cell overlap neighbors, so their batches must be
        // world-space depth bands drawn far to near (any basis, any layout). Otherwise
        // nothing overlaps and compact 32x32 cell chunks cull best.
        let cell = map.map.cell_size;
        let tall = map
            .map
            .definitions
            .values()
            .any(|d| !d.size.contains(&0.) && d.size != cell);
        let band = 32. * cell[1];
        let mut ranks: Vec<_> = map.map.layers.iter().map(|l| l.id.as_str()).collect();
        ranks.sort_unstable();
        for layer in &map.map.layers {
            let rank = ranks.binary_search(&layer.id.as_str()).unwrap_or(0) as u32 + 1;
            let mut groups = BTreeMap::<[i32; 2], (Vec<(f32, f32, Instance)>, [f32; 4])>::new();
            for (key, tile) in &layer.cells {
                let (x, y) = tilemap::coordinate(key)?;
                let definition = map
                    .map
                    .definition(*tile)
                    .ok_or_else(|| anyhow::anyhow!("unknown tile {tile}"))?;
                let [cx, cy] = map.map.layout.centre(x, y, map.map.cell_size);
                let b = map.basis;
                let position = [
                    map.origin[0] + b[0] * cx + b[1] * cy,
                    map.origin[1] + b[2] * cx + b[3] * cy,
                ];
                let draw = Draw {
                    position,
                    basis: b,
                    atlas,
                    normal: map.map.normal_atlas.as_ref().map(|n| n.0.as_str()),
                    region: &definition.region,
                    size: if definition.size.contains(&0.) {
                        map.map.cell_size
                    } else {
                        definition.size
                    },
                    tint: layer.tint,
                    parallax: layer.parallax,
                    flags: u32::from(definition.lit) << 1,
                };
                let instance = resolver.instance(&draw, map_id)?;
                // Culling bounds of the quad [-pivot, 1 - pivot] * size under the map basis.
                let [size, pivot] = [draw.size, [instance.size_pivot[2], instance.size_pivot[3]]];
                let offset = [(0.5 - pivot[0]) * size[0], (0.5 - pivot[1]) * size[1]];
                let centre = [
                    position[0] + b[0] * offset[0] + b[1] * offset[1],
                    position[1] + b[2] * offset[0] + b[3] * offset[1],
                ];
                let [hx, hy] = size.map(|v| v * 0.5);
                let ex = b[0].abs() * hx + b[1].abs() * hy;
                let ey = b[2].abs() * hx + b[3].abs() * hy;
                let rect = [
                    centre[0] - ex,
                    centre[1] - ey,
                    centre[0] + ex,
                    centre[1] + ey,
                ];
                let (instances, bounds) = groups
                    .entry(if tall {
                        [-(position[1] / band).floor() as i32, 0]
                    } else {
                        [x.div_euclid(32), y.div_euclid(32)]
                    })
                    .or_insert_with(|| (Vec::new(), rect));
                instances.push((position[1], position[0], instance));
                *bounds = [
                    bounds[0].min(rect[0]),
                    bounds[1].min(rect[1]),
                    bounds[2].max(rect[2]),
                    bounds[3].max(rect[3]),
                ];
            }
            for (chunk, (mut instances, bounds)) in groups {
                // Back to front: farther (higher) tiles first, then left to right.
                instances.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
                let instances = instances
                    .into_iter()
                    .map(|(.., instance)| instance)
                    .collect();
                let key = (layer.sorting_layer, layer.order, map.entity, rank, chunk);
                chunks.push((key, instances, (bounds, layer.parallax)));
            }
        }
    }
    chunks.sort_unstable_by_key(|(key, ..)| *key);
    // Merge the two key-sorted streams into draws in painter order.
    let mut result = Prepared {
        instances: Vec::with_capacity(sprites.len()),
        batches: Vec::new(),
        names,
    };
    let (mut s, mut c) = (
        sprites.into_iter().peekable(),
        chunks.into_iter().peekable(),
    );
    loop {
        let sprite_first = match (s.peek(), c.peek()) {
            (Some(a), Some(b)) => a.0 < b.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let first = u32::try_from(result.instances.len())?;
        if sprite_first {
            let (_, vertices, instance) = s.next().expect("peeked sprite");
            result.instances.push(instance);
            // Consecutive sprites with equal vertex counts share one procedural draw.
            if let Some(previous) = result.batches.last_mut()
                && previous.bounds.is_none()
                && previous.vertices == vertices
            {
                previous.count += 1;
                continue;
            }
            result.batches.push(Batch {
                first,
                count: 1,
                vertices,
                bounds: None,
            });
        } else {
            let (_, instances, bounds) = c.next().expect("peeked chunk");
            result.batches.push(Batch {
                first,
                count: u32::try_from(instances.len())?,
                vertices: 6,
                bounds: Some(bounds),
            });
            result.instances.extend(instances);
        }
    }
    ensure!(
        result.instances.len() <= 1_000_000,
        "sprite/tile extraction exceeds capacity"
    );
    Ok(result)
}
/// (atlas, normal atlas, region) references identifying one resolved draw.
type DrawKey<'a> = (&'a str, Option<&'a str>, &'a str);
/// Atlas data shared by every draw of one (atlas, normal, region) triple.
#[derive(Clone, Copy)]
struct Resolved {
    textures: [u32; 2],
    uv: [f32; 4],
    pivot: [f32; 2],
    borders: [f32; 4],
    source: [f32; 2],
}
/// Builds instances, memoizing atlas lookups: the last (atlas, normal, region) hit is
/// reused, and each color/normal pair's full region comparison runs once per prepare.
struct Resolver<'a> {
    scene: &'a Scene2d,
    indices: &'a BTreeMap<String, u32>,
    paired: Vec<(&'a str, &'a str)>,
    last: Option<(DrawKey<'a>, Resolved)>,
}
impl<'a> Resolver<'a> {
    fn new(scene: &'a Scene2d, indices: &'a BTreeMap<String, u32>) -> Self {
        Self {
            scene,
            indices,
            paired: Vec::new(),
            last: None,
        }
    }
    fn resolve(&mut self, draw: &Draw<'a>) -> Result<Resolved> {
        let atlas = self
            .scene
            .atlases
            .get(draw.atlas)
            .ok_or_else(|| anyhow::anyhow!("missing atlas {}", draw.atlas))?;
        let region = atlas
            .atlas
            .regions
            .get(draw.region)
            .ok_or_else(|| anyhow::anyhow!("missing sprite region {}", draw.region))?;
        ensure!(!atlas.linear, "color atlas must be sRGB");
        let normal = if let Some(normal) = draw.normal {
            let n = self
                .scene
                .atlases
                .get(normal)
                .ok_or_else(|| anyhow::anyhow!("missing normal atlas"))?;
            if !self.paired.contains(&(draw.atlas, normal)) {
                ensure!(
                    n.linear && n.atlas == atlas.atlas,
                    "normal atlas must use matching regions and linear encoding"
                );
                self.paired.push((draw.atlas, normal));
            }
            self.indices[normal]
        } else {
            self.indices["__flat_normal"]
        };
        let [w, h] = [atlas.atlas.width as f32, atlas.atlas.height as f32];
        let [rw, rh] = [region.width as f32, region.height as f32];
        Ok(Resolved {
            textures: [self.indices[draw.atlas], normal],
            uv: [region.x as f32 / w, region.y as f32 / h, rw / w, rh / h],
            pivot: region.pivot,
            borders: region.borders,
            source: [rw, rh],
        })
    }
    fn instance(&mut self, draw: &Draw<'a>, id: u32) -> Result<Instance> {
        ensure!(
            draw.position
                .iter()
                .chain(&draw.basis)
                .chain(&draw.size)
                .chain(&draw.tint)
                .chain(&draw.parallax)
                .all(|v| v.is_finite())
                && draw.size.iter().all(|v| *v >= 0.),
            "nonfinite sprite"
        );
        let key = (draw.atlas, draw.normal, draw.region);
        let r = match self.last {
            Some((last, resolved)) if last == key => resolved,
            _ => {
                let resolved = self.resolve(draw)?;
                self.last = Some((key, resolved));
                resolved
            }
        };
        Ok(Instance {
            pose: [draw.position[0], draw.position[1], 0., 0.],
            size_pivot: [draw.size[0], draw.size[1], r.pivot[0], r.pivot[1]],
            uv: r.uv,
            borders: r.borders,
            color: draw.tint,
            info: [r.textures[0], r.textures[1], id, draw.flags],
            parallax_source: [draw.parallax[0], draw.parallax[1], r.source[0], r.source[1]],
            basis: draw.basis,
        })
    }
}
