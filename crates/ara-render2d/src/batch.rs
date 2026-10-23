use crate::{Scene2d, scene::SpriteDraw};
use anyhow::{Result, ensure};
use ara_2d::{Sprite, tilemap};
use bytemuck::{Pod, Zeroable};
use std::collections::{BTreeMap, BTreeSet};

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
    pub bounds: Option<([f32; 4], [f32; 2])>,
}
struct Group {
    sort: (i32, i32, u64, String, [i32; 2]),
    instances: Vec<Instance>,
    bounds: Option<([f32; 4], [f32; 2])>,
    vertices: u32,
}
pub(crate) struct Prepared {
    pub instances: Vec<Instance>,
    pub batches: Vec<Batch>,
    pub names: Vec<(u64, String)>,
}

pub(crate) fn prepare(scene: &Scene2d, indices: &BTreeMap<String, u32>) -> Result<Prepared> {
    ensure!(
        scene.sprites.len() <= 1_000_000,
        "sprite extraction exceeds capacity"
    );
    let mut identities = BTreeMap::new();
    for sprite in &scene.sprites {
        ensure!(
            identities
                .insert(sprite.entity, sprite.name.to_owned())
                .is_none(),
            "duplicate entity identity"
        );
    }
    for map in &scene.tilemaps {
        ensure!(
            identities.insert(map.entity, map.name.to_owned()).is_none(),
            "duplicate entity identity"
        );
    }
    let names: Vec<_> = identities.into_iter().collect();
    let ids: BTreeMap<_, _> = names
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i as u32 + 1))
        .collect();
    let mut groups = Vec::new();
    for sprite in &scene.sprites {
        let vertices = if sprite.sprite.nine_slice { 54 } else { 6 };
        groups.push(Group {
            sort: (
                sprite.sprite.sorting_layer,
                sprite.sprite.order,
                sprite.entity,
                String::new(),
                [0; 2],
            ),
            instances: vec![instance(sprite, scene, indices, ids[&sprite.entity])?],
            bounds: None,
            vertices,
        });
    }
    for map in &scene.tilemaps {
        map.map.validate()?;
        for layer in &map.map.layers {
            let mut chunks = BTreeMap::<[i32; 2], Vec<Instance>>::new();
            let mut bounds = BTreeMap::<[i32; 2], [f32; 4]>::new();
            for (key, tile) in &layer.cells {
                let (x, y) = tilemap::coordinate(key)?;
                let chunk = [x.div_euclid(32), y.div_euclid(32)];
                let definition = &map.map.definitions[&tile.tile.to_string()];
                let centre = map.map.layout.centre(x, y, map.map.cell_size);
                let draw = SpriteDraw {
                    entity: map.entity,
                    name: String::new(),
                    position: [
                        map.origin[0] + map.basis[0] * centre[0] + map.basis[1] * centre[1],
                        map.origin[1] + map.basis[2] * centre[0] + map.basis[3] * centre[1],
                    ],
                    basis: map.basis,
                    sprite: Sprite {
                        atlas: map.map.atlas.to_owned(),
                        normal_atlas: map.map.normal_atlas.to_owned(),
                        region: definition.region.to_owned(),
                        size: map.map.cell_size,
                        lit: definition.lit,
                        parallax: layer.parallax,
                        ..Default::default()
                    },
                };
                chunks.entry(chunk).or_default().push(instance(
                    &draw,
                    scene,
                    indices,
                    ids[&map.entity],
                )?);
                let [sx, sy] = map.map.cell_size.map(|v| v * 0.5);
                let ex = map.basis[0].abs() * sx + map.basis[1].abs() * sy;
                let ey = map.basis[2].abs() * sx + map.basis[3].abs() * sy;
                let rect = [
                    draw.position[0] - ex,
                    draw.position[1] - ey,
                    draw.position[0] + ex,
                    draw.position[1] + ey,
                ];
                bounds
                    .entry(chunk)
                    .and_modify(|b| {
                        b[0] = b[0].min(rect[0]);
                        b[1] = b[1].min(rect[1]);
                        b[2] = b[2].max(rect[2]);
                        b[3] = b[3].max(rect[3]);
                    })
                    .or_insert(rect);
            }
            for (chunk, instances) in chunks {
                groups.push(Group {
                    sort: (
                        layer.sorting_layer,
                        layer.order,
                        map.entity,
                        layer.id.to_owned(),
                        chunk,
                    ),
                    instances,
                    bounds: Some((bounds[&chunk], layer.parallax)),
                    vertices: 6,
                });
            }
        }
    }
    groups.sort_by(|a, b| a.sort.cmp(&b.sort));
    let mut result = Prepared {
        instances: Vec::new(),
        batches: Vec::new(),
        names,
    };
    let mut seen = BTreeSet::new();
    for group in groups {
        ensure!(seen.insert(group.sort), "ambiguous render sort key");
        let first = u32::try_from(result.instances.len())?;
        let count = u32::try_from(group.instances.len())?;
        result.instances.extend(group.instances);
        if group.bounds.is_none()
            && let Some(previous) = result.batches.last_mut()
            && previous.bounds.is_none()
            && previous.vertices == group.vertices
        {
            previous.count += count;
            continue;
        }
        result.batches.push(Batch {
            first,
            count,
            vertices: group.vertices,
            bounds: group.bounds,
        });
    }
    ensure!(
        result.instances.len() <= 1_000_000,
        "sprite/tile extraction exceeds capacity"
    );
    Ok(result)
}
fn instance(
    draw: &SpriteDraw,
    scene: &Scene2d,
    indices: &BTreeMap<String, u32>,
    id: u32,
) -> Result<Instance> {
    let sprite = &draw.sprite;
    ensure!(
        draw.position
            .iter()
            .chain(draw.basis.iter())
            .chain(sprite.size.iter())
            .chain(sprite.tint.iter())
            .chain(sprite.parallax.iter())
            .all(|v| v.is_finite())
            && sprite.size.iter().all(|v| *v >= 0.),
        "nonfinite sprite"
    );
    let asset = &sprite
        .atlas
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("sprite has no atlas"))?
        .0;
    let atlas = scene
        .atlases
        .get(asset)
        .ok_or_else(|| anyhow::anyhow!("missing atlas {asset}"))?;
    let region = atlas
        .atlas
        .regions
        .get(&sprite.region)
        .ok_or_else(|| anyhow::anyhow!("missing sprite region {}", sprite.region))?;
    ensure!(!atlas.linear, "color atlas must be sRGB");
    let normal = if let Some(normal) = &sprite.normal_atlas {
        let n = scene
            .atlases
            .get(&normal.0)
            .ok_or_else(|| anyhow::anyhow!("missing normal atlas"))?;
        ensure!(
            n.linear && n.atlas == atlas.atlas,
            "normal atlas must use matching regions and linear encoding"
        );
        indices[&normal.0]
    } else {
        indices["__flat_normal"]
    };
    let flags = u32::from(sprite.nine_slice)
        | (u32::from(sprite.lit) << 1)
        | (u32::from(sprite.flip_x) << 2)
        | (u32::from(sprite.flip_y) << 3);
    Ok(Instance {
        pose: [draw.position[0], draw.position[1], 0., 0.],
        size_pivot: [
            sprite.size[0],
            sprite.size[1],
            region.pivot[0],
            region.pivot[1],
        ],
        uv: [
            region.x as f32 / atlas.atlas.width as f32,
            region.y as f32 / atlas.atlas.height as f32,
            region.width as f32 / atlas.atlas.width as f32,
            region.height as f32 / atlas.atlas.height as f32,
        ],
        borders: region.borders,
        color: sprite.tint,
        info: [indices[asset], normal, id, flags],
        parallax_source: [
            sprite.parallax[0],
            sprite.parallax[1],
            region.width as f32,
            region.height as f32,
        ],
        basis: draw.basis,
    })
}
