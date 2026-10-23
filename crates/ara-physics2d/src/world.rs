//! The solver resource: complete Rapier state, stable-identity bindings and checkpoints.
use crate::{CharacterController2d, Collider2d, RigidBody2d};
use anyhow::{Result, ensure};
use ara_schema::{FieldType, Schema, ValueType};
use ara_sim::bevy_ecs::prelude::Resource;
use bincode::Options;
use rapier2d::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::collections::BTreeMap;

/// Immutable world settings; a snapshot only restores into an identical configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Physics2dConfig {
    /// Fixed tick frequency; must equal the session's tick rate.
    pub tick_hz: u32,
    /// Gravity in metres per second squared.
    pub gravity: [f32; 2],
    /// Symmetric inclusion masks for sixteen layers; static layer zero does not self-collide.
    pub layer_masks: [u32; 16],
    /// Velocity solver iterations per tick.
    pub solver_iterations: u32,
}
impl Default for Physics2dConfig {
    fn default() -> Self {
        let mut layer_masks = [0xffff; 16];
        layer_masks[0] = 0xfffe;
        Self {
            tick_hz: 60,
            gravity: [0., -9.81],
            layer_masks,
            solver_iterations: 4,
        }
    }
}
impl Physics2dConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=1000).contains(&self.tick_hz)
                && self.gravity.iter().all(|v| v.is_finite())
                && (1..=64).contains(&self.solver_iterations),
            "invalid 2D physics tick rate, gravity or iteration count"
        );
        for (a, mask) in self.layer_masks.iter().enumerate() {
            ensure!(*mask <= 0xffff, "layer masks cover sixteen layers");
            for b in 0..16 {
                ensure!(
                    (mask >> b & 1) == (self.layer_masks[b] >> a & 1),
                    "layer masks must be symmetric"
                );
            }
        }
        Ok(())
    }
    /// The world's up direction, opposite gravity; None in zero-gravity worlds.
    pub(crate) fn up(&self) -> Option<Vector> {
        let gravity = Vector::new(self.gravity[0], self.gravity[1]);
        (gravity.length() > 0.).then(|| -gravity.normalize())
    }
}

/// A stable-identity query result, independent of Rapier's native handle layout.
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit2d {
    /// Stable source entity identity.
    pub entity: String,
    /// World-space intersection point in metres.
    pub point: [f32; 2],
    /// World-space outward unit normal.
    pub normal: [f32; 2],
    /// Distance from the ray origin in metres.
    pub distance: f32,
}

/// Authoring configuration a binding was created from; any change recreates it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum Source {
    Body(RigidBody2d, Collider2d),
    Character(CharacterController2d),
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Binding {
    pub body: RigidBodyHandle,
    pub collider: ColliderHandle,
    pub source: Source,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct SolverState {
    pub config: Physics2dConfig,
    pub world: PhysicsWorld,
    /// Stable identity to native objects, iterated in identity order.
    pub bindings: BTreeMap<String, Binding>,
    /// Raw collider handle to identity, for query and contact results.
    pub owners: BTreeMap<(u32, u32), String>,
}
impl FieldType for SolverState {
    fn value_type() -> ValueType {
        ValueType::List(Box::new(ValueType::U8))
    }
}

/// Bounded fixed-width codec; trailing bytes are rejected.
fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(256 * 1024 * 1024)
        .reject_trailing_bytes()
}
/// Build fingerprint: snapshots never cross Rapier versions, precision or platforms.
fn build() -> String {
    format!(
        "ARA2P2:rapier-{}:f32:enhanced:serial:{}:{}\0",
        rapier2d::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
impl SolverState {
    fn bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = build().into_bytes();
        bytes.extend(codec().serialize(self)?);
        Ok(bytes)
    }
    fn read(bytes: &[u8]) -> Result<Self> {
        let build = build();
        ensure!(
            bytes.len() <= 256 * 1024 * 1024 && bytes.starts_with(build.as_bytes()),
            "2D physics snapshot build/size mismatch"
        );
        let state: Self = codec().deserialize(&bytes[build.len()..])?;
        state.config.validate()?;
        Ok(state)
    }
}
mod state_serde {
    use super::*;
    pub fn serialize<S: Serializer>(state: &SolverState, serializer: S) -> Result<S::Ok, S::Error> {
        state
            .bytes()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SolverState, D::Error> {
        SolverState::read(&Vec::<u8>::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Solver infrastructure, registered with ara-sim so session checkpoints carry the
/// complete contact/island/broad-phase state. Gameplay state stays in components.
#[derive(Resource, Schema, Serialize, Deserialize)]
pub struct Physics2d {
    /// Complete versioned backend state; serialized only for checkpoints and hashes.
    #[serde(with = "state_serde")]
    pub(crate) state: SolverState,
}
impl Physics2d {
    /// Creates a deterministic single-threaded solver.
    pub fn new(config: Physics2dConfig) -> Result<Self> {
        config.validate()?;
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(config.gravity[0], config.gravity[1]);
        world.integration_parameters.dt = 1. / config.tick_hz as f32;
        world.integration_parameters.num_solver_iterations = config.solver_iterations as usize;
        Ok(Self {
            state: SolverState {
                config,
                world,
                bindings: BTreeMap::new(),
                owners: BTreeMap::new(),
            },
        })
    }
    /// Immutable world settings.
    pub fn config(&self) -> &Physics2dConfig {
        &self.state.config
    }
    /// Number of bound bodies and characters.
    pub fn len(&self) -> usize {
        self.state.bindings.len()
    }
    /// True before the first boundary binds any entity.
    pub fn is_empty(&self) -> bool {
        self.state.bindings.is_empty()
    }
    pub(crate) fn owner(&self, collider: ColliderHandle) -> Option<&str> {
        self.state
            .owners
            .get(&collider.into_raw_parts())
            .map(String::as_str)
    }
    /// Casts a finite ray against the last completed tick's geometry. Sensor participation
    /// is explicit. A zero-length direction is rejected.
    pub fn raycast(
        &self,
        origin: [f32; 2],
        direction: [f32; 2],
        max_distance: f32,
        include_sensors: bool,
    ) -> Result<Option<RayHit2d>> {
        ensure!(
            origin.iter().chain(direction.iter()).all(|v| v.is_finite())
                && max_distance.is_finite()
                && max_distance >= 0.,
            "invalid 2D ray"
        );
        let length = direction[0].hypot(direction[1]);
        ensure!(
            length.is_finite() && length > 0.,
            "zero/overflowing ray direction"
        );
        let ray = Ray::new(
            Vector::new(origin[0], origin[1]),
            Vector::new(direction[0] / length, direction[1] / length),
        );
        let filter = if include_sensors {
            QueryFilter::default()
        } else {
            QueryFilter::default().exclude_sensors()
        };
        let query = self.state.world.query_pipeline_with_filter(filter);
        Ok(query
            .cast_ray_and_get_normal(&ray, max_distance, true)
            .and_then(|(collider, hit)| {
                let point = ray.origin + ray.dir * hit.time_of_impact;
                self.owner(collider).map(|entity| RayHit2d {
                    entity: entity.to_owned(),
                    point: [point.x, point.y],
                    normal: [hit.normal.x, hit.normal.y],
                    distance: hit.time_of_impact,
                })
            }))
    }
    /// Identities touching `entity` after the last tick: solid contacts and sensor
    /// overlaps, sorted and deduplicated. Use for triggers, pickups and hazards.
    pub fn touching(&self, entity: &str) -> Vec<&str> {
        let Some(binding) = self.state.bindings.get(entity) else {
            return Vec::new();
        };
        let phase = &self.state.world.narrow_phase;
        let contacts = phase
            .contact_pairs_with(binding.collider)
            .filter(|pair| pair.has_any_active_contact())
            .map(|pair| {
                if pair.collider1 == binding.collider {
                    pair.collider2
                } else {
                    pair.collider1
                }
            });
        let overlaps = phase
            .intersection_pairs_with(binding.collider)
            .filter(|(_, _, intersecting)| *intersecting)
            .map(|(a, b, _)| if a == binding.collider { b } else { a });
        let mut result: Vec<_> = contacts
            .chain(overlaps)
            .filter_map(|collider| self.owner(collider))
            .collect();
        result.sort_unstable();
        result.dedup();
        result
    }
    /// Complete solver checkpoint for same-build netcode rollback and save games.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        self.state.bytes()
    }
    /// Restores atomically; the existing solver survives malformed or mismatched data.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<()> {
        let state = SolverState::read(bytes)?;
        ensure!(
            state.config == self.state.config,
            "2D physics snapshot configuration mismatch"
        );
        self.state = state;
        Ok(())
    }
}
