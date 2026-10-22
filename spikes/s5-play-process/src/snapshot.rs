//! Columnar binary world snapshot, used by the "wsBinary" and "shm" transports.
//!
//! Layout (all little-endian):
//! ```text
//! "ARASNAP1" | version u32 | entity_count u32 | next_id u32 | column_count u32
//! scene ids: entity_count × u32, ascending
//! per column: name_len u16 | name | payload_len u64 | payload
//!   payload = presence bitset (ceil(n/8) bytes) | present values in entity order
//! ```
//! Entities are in stable-ID order, so equal worlds encode to identical bytes.

use anyhow::{Result, bail};
use bevy_ecs::prelude::*;

use crate::model::{IdIndex, NextSceneId, SceneId, sorted_entities};
use crate::registry::{COMPONENTS, component, take};

const MAGIC: &[u8; 8] = b"ARASNAP1";
const VERSION: u32 = 1;

/// Appends the snapshot of every entity with a stable ID to `out`.
pub fn encode_into(world: &mut World, out: &mut Vec<u8>) {
    let rows = sorted_entities(world);
    let entities: Vec<Entity> = rows.iter().map(|r| r.1).collect();
    out.reserve(rows.len() * 120);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    out.extend_from_slice(&world.resource::<NextSceneId>().0.to_le_bytes());
    out.extend_from_slice(&(COMPONENTS.len() as u32).to_le_bytes());
    for (id, _) in &rows {
        out.extend_from_slice(&id.0.to_le_bytes());
    }
    for kind in &COMPONENTS {
        out.extend_from_slice(&(kind.name.len() as u16).to_le_bytes());
        out.extend_from_slice(kind.name.as_bytes());
        let len_at = out.len();
        out.extend_from_slice(&[0; 8]);
        (kind.encode_column)(world, &entities, out);
        let len = (out.len() - len_at - 8) as u64;
        out[len_at..len_at + 8].copy_from_slice(&len.to_le_bytes());
    }
}

/// Encodes into a fresh buffer.
pub fn encode(world: &mut World) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(world, &mut out);
    out
}

fn read_u32(input: &mut &[u8]) -> Result<u32> {
    Ok(u32::from_le_bytes(take(input, 4)?.try_into()?))
}

/// Builds a fresh world from a snapshot: one batch spawn, then one batch insert per column.
pub fn decode(bytes: &[u8]) -> Result<World> {
    let mut input = bytes;
    if take(&mut input, 8)? != MAGIC {
        bail!("not an Ara snapshot");
    }
    let version = read_u32(&mut input)?;
    if version != VERSION {
        bail!("snapshot version {version}, expected {VERSION}");
    }
    let count = read_u32(&mut input)? as usize;
    let next_id = read_u32(&mut input)?;
    let columns = read_u32(&mut input)?;
    let ids: Vec<SceneId> = take(&mut input, count * 4)?
        .chunks_exact(4)
        .map(|c| SceneId(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .collect();

    let mut world = World::new();
    let entities: Vec<Entity> = world.spawn_batch(ids.iter().copied()).collect();
    for _ in 0..columns {
        let name_len = u16::from_le_bytes(take(&mut input, 2)?.try_into()?) as usize;
        let name = std::str::from_utf8(take(&mut input, name_len)?)?;
        let len = u64::from_le_bytes(take(&mut input, 8)?.try_into()?) as usize;
        let payload = take(&mut input, len)?;
        (component(name)?.decode_column)(&mut world, &entities, payload)?;
    }
    if !input.is_empty() {
        bail!("snapshot has {} trailing bytes", input.len());
    }
    world.insert_resource(IdIndex(ids.into_iter().zip(entities).collect()));
    world.insert_resource(NextSceneId(next_id));
    Ok(world)
}
