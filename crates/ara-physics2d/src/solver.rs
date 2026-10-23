pub(crate) use crate::shape::validate;
use crate::shape::{body_builder, collider_builder};
use crate::{Body2d, BodyKind2d, Character2d, Collider2d};
use anyhow::{Result, ensure};
use ara_schema::{FieldType, Schema, ValueType};
use ara_sim::{Identity, SimContext, bevy_ecs::prelude::*};
use bincode::Options;
use rapier2d::{
    control::{CharacterAutostep, CharacterLength, KinematicCharacterController},
    prelude::*,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::collections::{BTreeMap, BTreeSet};

fn build() -> String {
    format!(
        "ARA2P1:rapier-{}:f32:enhanced:serial:{}:{}\0",
        rapier2d::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
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
    /// Distance from ray origin in metres.
    pub distance: f32,
}
#[derive(Serialize, Deserialize)]
struct Binding {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    config: Collider2d,
}
#[derive(Serialize, Deserialize)]
struct SolverState {
    world: PhysicsWorld,
    bindings: BTreeMap<String, Binding>,
}
impl FieldType for SolverState {
    fn value_type() -> ValueType {
        ValueType::List(Box::new(ValueType::U8))
    }
}
fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(256 * 1024 * 1024)
        .reject_trailing_bytes()
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
            bytes.starts_with(build.as_bytes()),
            "physics snapshot build mismatch"
        );
        Ok(codec().deserialize(&bytes[build.len()..])?)
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

/// Solver infrastructure. The world/contact cache is fully registered for rollback.
#[derive(Resource, Schema, Serialize, Deserialize)]
pub struct Physics2d {
    /// Complete versioned backend state; serialized only for checkpoints/hashes.
    #[serde(with = "state_serde")]
    state: SolverState,
    /// Fixed tick frequency; rejects changing simulation dt behind the solver.
    hz: u32,
    /// Last rejected tick. A host must stop play and surface this diagnostic.
    error: Option<String>,
}
impl Physics2d {
    /// Creates a deterministic single-threaded solver, without foreign build tools.
    pub fn new(hz: u32, gravity: [f32; 2]) -> Result<Self> {
        ensure!(
            (1..=1000).contains(&hz) && gravity.iter().all(|v| v.is_finite()),
            "invalid 2D physics configuration"
        );
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(gravity[0], gravity[1]);
        world.integration_parameters.dt = 1. / hz as f32;
        Ok(Self {
            state: SolverState {
                world,
                bindings: BTreeMap::new(),
            },
            hz,
            error: None,
        })
    }
    /// Last boundary failure, for host play-control diagnostics.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    /// Casts a finite normalized ray against the last completed tick's geometry.
    /// Sensor participation is explicit. A zero-length direction is rejected.
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
        let direction = Vector::new(direction[0] / length, direction[1] / length);
        let ray = Ray::new(Vector::new(origin[0], origin[1]), direction);
        let filter = if include_sensors {
            QueryFilter::default()
        } else {
            QueryFilter::default().exclude_sensors()
        };
        let query = self.state.world.query_pipeline_with_filter(filter);
        Ok(query
            .cast_ray_and_get_normal(&ray, max_distance, true)
            .and_then(|(collider, hit)| {
                self.state
                    .bindings
                    .iter()
                    .find(|(_, binding)| binding.collider == collider)
                    .map(|(id, _)| {
                        let point = ray.origin + ray.dir * hit.time_of_impact;
                        RayHit2d {
                            entity: id.to_owned(),
                            point: [point.x, point.y],
                            normal: [hit.normal.x, hit.normal.y],
                            distance: hit.time_of_impact,
                        }
                    })
            }))
    }
    /// Complete solver checkpoint, suitable for same-build netcode rollback.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        let mut bytes = b"ARA2S\x01\0\0".to_vec();
        bytes.extend(self.hz.to_le_bytes());
        let error = self.error.as_deref().unwrap_or("");
        ensure!(
            error.len() < 1024 * 1024,
            "physics diagnostic exceeds snapshot limit"
        );
        bytes.extend(
            (if self.error.is_some() {
                error.len() as u32 + 1
            } else {
                0
            })
            .to_le_bytes(),
        );
        bytes.extend(error.as_bytes());
        bytes.extend(self.state.bytes()?);
        Ok(bytes)
    }
    /// Restores atomically. Existing solver survives malformed/incompatible data.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() >= 16
                && bytes.len() <= 256 * 1024 * 1024
                && &bytes[..8] == b"ARA2S\x01\0\0",
            "invalid physics snapshot header/size"
        );
        let hz = u32::from_le_bytes(bytes[8..12].try_into()?);
        let error_len = u32::from_le_bytes(bytes[12..16].try_into()?);
        let end = 16 + error_len.saturating_sub(1) as usize;
        ensure!(
            end <= bytes.len() && error_len <= 1024 * 1024,
            "invalid physics snapshot diagnostic"
        );
        let error = if error_len == 0 {
            None
        } else {
            Some(std::str::from_utf8(&bytes[16..end])?.to_owned())
        };
        let replacement = Self {
            state: SolverState::read(&bytes[end..])?,
            hz,
            error,
        };
        ensure!(
            replacement.hz == self.hz,
            "physics snapshot tick frequency mismatch"
        );
        *self = replacement;
        Ok(())
    }
    /// Synchronizes components, steps once, then writes poses/velocities back to ECS.
    /// Run only at a tick boundary, with all gameplay producers before this call.
    pub fn step(&mut self, ecs: &mut World, dt: f64) -> Result<()> {
        ensure!(
            self.error.is_none(),
            "physics service is faulted; restore a checkpoint"
        );
        ensure!(
            (dt - 1. / f64::from(self.hz)).abs() < 1e-9,
            "physics requires its configured fixed tick"
        );
        let mut query = ecs.query::<(
            Entity,
            &Identity,
            &Body2d,
            &Collider2d,
            Option<&Character2d>,
        )>();
        let mut rows: Vec<_> = query.iter(ecs).collect();
        rows.sort_by(|a, b| a.1.cmp(b.1));
        let mut ids = BTreeSet::new();
        for (_, id, body, collider, character) in &rows {
            ensure!(ids.insert(id.0.as_str()), "duplicate physics identity");
            validate(body, collider, *character)?;
        }
        let state = &mut self.state;
        let removed: Vec<_> = state
            .bindings
            .keys()
            .filter(|id| !ids.contains(id.as_str()))
            .map(String::to_owned)
            .collect();
        for id in removed {
            if let Some(binding) = state.bindings.remove(&id) {
                state.world.remove_body(binding.body);
            }
        }
        let mut changed = false;
        for (_, id, body, collider, _) in &rows {
            if !state.bindings.contains_key(&id.0) {
                let (bh, ch) = state
                    .world
                    .insert(body_builder(body), collider_builder(collider)?);
                state.bindings.insert(
                    id.0.to_owned(),
                    Binding {
                        body: bh,
                        collider: ch,
                        config: (*collider).to_owned(),
                    },
                );
                changed = true;
            }
            let binding = state.bindings.get_mut(&id.0).expect("inserted above");
            if binding.config != **collider {
                state.world.colliders.remove(
                    binding.collider,
                    &mut state.world.islands,
                    &mut state.world.bodies,
                    &mut state.world.soft_bodies,
                    true,
                );
                binding.collider = state.world.colliders.insert_with_parent(
                    collider_builder(collider)?,
                    binding.body,
                    &mut state.world.bodies,
                );
                binding.config = (*collider).to_owned();
                changed = true;
            }
            let rb = &mut state.world.bodies[binding.body];
            let kind = match body.kind {
                BodyKind2d::Fixed => RigidBodyType::Fixed,
                BodyKind2d::Dynamic => RigidBodyType::Dynamic,
                BodyKind2d::Kinematic => RigidBodyType::KinematicPositionBased,
            };
            if rb.body_type() != kind {
                rb.set_body_type(kind, true);
            }
            let position = Vector::new(body.position[0], body.position[1]);
            let pose = Pose::new(position, body.rotation);
            if *rb.position() != pose {
                if body.kind == BodyKind2d::Kinematic {
                    rb.set_next_kinematic_position(pose);
                } else {
                    rb.set_position(pose, true);
                }
                changed = true;
            }
            let velocity = Vector::new(body.velocity[0], body.velocity[1]);
            if rb.linvel() != velocity {
                rb.set_linvel(velocity, true);
            }
            if rb.angvel() != body.angular_velocity {
                rb.set_angvel(body.angular_velocity, true);
            }
            rb.lock_rotations(body.lock_rotation, false);
            rb.enable_ccd(body.ccd);
        }
        if changed {
            state.world.detect_collisions(&(), &());
        }
        let mut characters = Vec::new();
        for (entity, id, _, _, character) in &rows {
            if let Some(character) = character {
                let binding = &state.bindings[&id.0];
                let collider = &state.world.colliders[binding.collider];
                let controller = KinematicCharacterController {
                    max_slope_climb_angle: character.max_slope,
                    min_slope_slide_angle: character.max_slope,
                    autostep: (character.step_height > 0.).then_some(CharacterAutostep {
                        max_height: CharacterLength::Absolute(character.step_height),
                        min_width: CharacterLength::Absolute(0.05),
                        include_dynamic_bodies: false,
                    }),
                    snap_to_ground: (character.snap > 0.)
                        .then_some(CharacterLength::Absolute(character.snap)),
                    ..Default::default()
                };
                let queries = state.world.query_pipeline_with_filter(
                    QueryFilter::default()
                        .exclude_rigid_body(binding.body)
                        .exclude_sensors(),
                );
                let movement = controller.move_shape(
                    dt as f32,
                    &queries,
                    collider.shape(),
                    collider.position(),
                    Vector::new(character.translation[0], character.translation[1]),
                    |_| {},
                );
                let rb = &mut state.world.bodies[binding.body];
                let mut pose = *rb.position();
                pose.translation += movement.translation;
                rb.set_next_kinematic_position(pose);
                characters.push((*entity, movement.grounded));
            }
        }
        state.world.step();
        let results: Vec<_> = rows
            .iter()
            .map(|(entity, id, body, _, _)| {
                let rb = &state.world.bodies[state.bindings[&id.0].body];
                let p = rb.translation();
                let v = rb.linvel();
                (
                    *entity,
                    Body2d {
                        position: [p.x, p.y],
                        rotation: rb.rotation().angle(),
                        velocity: [v.x, v.y],
                        angular_velocity: rb.angvel(),
                        ..**body
                    },
                )
            })
            .collect();
        drop(rows);
        for (entity, body) in results {
            *ecs.get_mut::<Body2d>(entity)
                .expect("queried body survives exclusive boundary") = body;
        }
        for (entity, grounded) in characters {
            let mut character = ecs
                .get_mut::<Character2d>(entity)
                .expect("queried character survives boundary");
            character.grounded = grounded;
            character.translation = [0.; 2];
        }
        Ok(())
    }
}
/// Exclusive ara-sim system. Register after gameplay movement producers.
pub fn step_system(world: &mut World) {
    let dt = world.resource::<SimContext>().dt();
    world.resource_scope(|world, mut service: Mut<Physics2d>| {
        if let Err(error) = service.step(world, dt) {
            service.error = Some(error.to_string());
        }
    });
}
