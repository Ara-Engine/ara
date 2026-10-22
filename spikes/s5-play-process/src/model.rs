//! The play-mode world model shared by the editor host and the player: components, the
//! deterministic orbit simulation, the stable-ID index and the canonical state hash.

use std::collections::HashMap;

use anyhow::{Result, bail};
use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use xxhash_rust::xxh3::Xxh3;

use crate::registry::COMPONENTS;

/// Fixed simulation rate in ticks per second.
pub const TICK_HZ: u32 = 60;
/// Fixed simulation step in seconds. Simulation code never reads the wall clock.
pub const DT: f32 = 1.0 / TICK_HZ as f32;

/// Stable authoring identity of an entity (an A3 short ID). It is the only entity address that
/// crosses the process boundary; runtime `Entity` handles never leave a process.
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Pod, Zeroable)]
#[repr(transparent)]
pub struct SceneId(pub u32);

impl SceneId {
    /// Lower-case base-36, zero-padded to four digits so string order equals numeric order.
    pub fn key(self) -> String {
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut n = self.0;
        let mut out = Vec::with_capacity(8);
        while n > 0 || out.len() < 4 {
            out.push(DIGITS[(n % 36) as usize]);
            n /= 36;
        }
        out.reverse();
        String::from_utf8(out).expect("base-36 digits are ASCII")
    }

    /// Parses a key produced by [`SceneId::key`].
    pub fn parse(key: &str) -> Result<Self> {
        match u32::from_str_radix(key, 36) {
            Ok(n) => Ok(Self(n)),
            Err(_) => bail!("invalid scene id {key:?}"),
        }
    }
}

impl Serialize for SceneId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.key())
    }
}

impl<'de> Deserialize<'de> for SceneId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let key = String::deserialize(d)?;
        SceneId::parse(&key).map_err(serde::de::Error::custom)
    }
}

/// World-space placement. `position` is in metres, `rotation` is a unit quaternion (x, y, z, w).
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Pod, Zeroable)]
#[repr(C)]
pub struct Transform {
    /// Position in metres.
    pub position: [f32; 3],
    /// Unit quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// Per-axis scale factor.
    pub scale: [f32; 3],
}

/// Circular motion around `center` in the XZ plane, advanced by [`orbit_system`].
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Pod, Zeroable)]
#[repr(C)]
pub struct Orbit {
    /// Orbit centre in metres.
    pub center: [f32; 3],
    /// Orbit radius in metres.
    pub radius: f32,
    /// Angular speed in radians per second.
    pub angular_speed: f32,
    /// Current angle in radians. Simulation state: it changes every tick.
    pub phase: f32,
}

/// Linear RGBA colour, each channel in 0..=1.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Pod, Zeroable)]
#[repr(C)]
pub struct Tint {
    /// Red, green, blue, alpha.
    pub rgba: [f32; 4],
}

/// Display name shown in the hierarchy.
#[derive(Component, Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Label(pub String);

/// Number of simulation ticks run since play started (tick 0 is the snapshot).
#[derive(Resource, Clone, Copy, Default)]
pub struct SimTick(pub u64);

/// Next free [`SceneId`]; runtime spawns allocate from it deterministically.
#[derive(Resource, Clone, Copy)]
pub struct NextSceneId(pub u32);

/// Maps stable IDs to this process's runtime handles. Maintained by every spawn/despawn path.
#[derive(Resource, Default)]
pub struct IdIndex(pub HashMap<SceneId, Entity>);

/// Advances every orbiter by one fixed step. Each entity is independent, so the result does not
/// depend on query iteration order.
pub fn orbit_system(mut q: Query<(&mut Orbit, &mut Transform)>) {
    for (mut orbit, mut transform) in &mut q {
        orbit.phase = (orbit.phase + orbit.angular_speed * DT) % std::f32::consts::TAU;
        let (s, c) = orbit.phase.sin_cos();
        transform.position = [
            orbit.center[0] + orbit.radius * c,
            orbit.center[1] + 0.25 * (2.0 * orbit.phase).sin(),
            orbit.center[2] + orbit.radius * s,
        ];
        let (hs, hc) = (0.5 * orbit.phase).sin_cos();
        transform.rotation = [0.0, hs, 0.0, hc];
    }
}

/// A play world: the ECS world plus its fixed-step schedule.
pub struct Sim {
    pub world: World,
    schedule: Schedule,
}

impl Sim {
    /// Wraps a world that already holds entities, [`IdIndex`] and [`NextSceneId`].
    pub fn new(mut world: World) -> Self {
        world.insert_resource(SimTick(0));
        let mut schedule = Schedule::default();
        schedule.add_systems(orbit_system);
        Self { world, schedule }
    }

    /// Runs exactly one fixed tick.
    pub fn tick(&mut self) {
        self.schedule.run(&mut self.world);
        self.world.resource_mut::<SimTick>().0 += 1;
    }

    pub fn tick_count(&self) -> u64 {
        self.world.resource::<SimTick>().0
    }
}

/// Live entities ordered by stable ID: the canonical order for hashing and snapshots.
pub fn sorted_entities(world: &mut World) -> Vec<(SceneId, Entity)> {
    let mut rows: Vec<_> = world
        .query::<(Entity, &SceneId)>()
        .iter(world)
        .map(|(e, id)| (*id, e))
        .collect();
    rows.sort_unstable_by_key(|r| r.0);
    rows
}

/// Canonical hash of all registered component state, in stable-ID order. Two worlds with equal
/// hashes hold bit-identical component values for the same IDs.
pub fn state_hash(world: &mut World) -> u64 {
    let rows = sorted_entities(world);
    let mut h = Xxh3::new();
    let mut buf = Vec::with_capacity(256);
    for (id, e) in rows {
        buf.clear();
        buf.extend_from_slice(&id.0.to_le_bytes());
        for (tag, kind) in COMPONENTS.iter().enumerate() {
            if (kind.write_bytes)(world, e, &mut buf) {
                buf.push(tag as u8);
            }
        }
        h.update(&buf);
    }
    h.digest()
}

/// Number of entities with a stable ID.
pub fn entity_count(world: &mut World) -> usize {
    world.query::<&SceneId>().iter(world).count()
}
