//! Fixed-tick ECS boundary: extract in stable Identity order, step once, write back.
use crate::{
    CharacterController2d, CharacterInput2d, CharacterMotion2d, Collider2d, MotionType2d,
    Physics2d, PhysicsPose2d, RigidBody2d, Velocity2d,
    shape::{self, body_builder, character_collider, collider_builder},
    world::{Binding, SolverState, Source},
};
use anyhow::{Result, ensure};
use ara_schema::Schema;
use ara_sim::{Identity, Session, SimContext, bevy_ecs::prelude::*};
use rapier2d::{
    control::{CharacterAutostep, CharacterLength, KinematicCharacterController},
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// Sticky fault from the physics system. The host pauses that session and reports the
/// error; restoring a checkpoint taken before the fault clears it (the status is
/// registered with ara-sim, so checkpoints carry it).
#[derive(Debug, Default, Resource, Schema, Serialize, Deserialize)]
pub struct Physics2dStatus {
    /// Last failure; while set, the system skips ticks.
    pub error: Option<String>,
}

/// Inserts the solver and status, then appends the boundary system to the session.
/// Install after gameplay systems that write inputs, poses or velocities.
pub fn install(session: &mut Session, config: crate::Physics2dConfig) -> Result<()> {
    let hz = config.tick_hz;
    let physics = Physics2d::new(config)?;
    let world = session.world_mut();
    ensure!(
        !world.contains_resource::<Physics2d>(),
        "2D physics is already installed in this session"
    );
    if let Some(context) = world.get_resource::<SimContext>() {
        ensure!(
            (context.dt() - 1. / f64::from(hz)).abs() < 1e-9,
            "2D physics tick rate must equal the session's fixed tick"
        );
    }
    world.insert_resource(physics);
    world.init_resource::<Physics2dStatus>();
    Ok(session.add_system("physics2d", step_system)?)
}

/// Exclusive ara-sim system; records failures in `Physics2dStatus` instead of panicking.
pub fn step_system(world: &mut World) {
    if world
        .get_resource::<Physics2dStatus>()
        .is_some_and(|s| s.error.is_some())
    {
        return;
    }
    if let Err(error) = tick_ecs(world) {
        world.get_resource_or_init::<Physics2dStatus>().error = Some(format!("{error:#}"));
    }
}

/// Runs one boundary directly, returning errors instead of storing a fault.
/// Validates the whole extraction before any solver mutation.
pub fn tick_ecs(world: &mut World) -> Result<()> {
    world.resource_scope(|world, mut physics: Mut<Physics2d>| run(world, &mut physics.state))
}

/// Binds lifecycle/configuration changes without advancing time, so queries and an
/// initial checkpoint see the projected scene before the first tick.
pub fn prepare_ecs(world: &mut World) -> Result<()> {
    world.resource_scope(|world, mut physics: Mut<Physics2d>| {
        let rows = rows(world)?;
        let created = synchronize(world, &mut physics.state, &rows)?;
        refresh_queries(&mut physics.state, &created);
        Ok(())
    })
}

struct Row {
    entity: Entity,
    id: String,
    source: Source,
    pose: PhysicsPose2d,
    velocity: Velocity2d,
    input: CharacterInput2d,
    /// Previous boundary's CharacterMotion2d support, read from ECS state.
    grounded: bool,
}

fn rows(world: &mut World) -> Result<Vec<Row>> {
    let mut query = world.query_filtered::<(
        Entity,
        &Identity,
        Option<&RigidBody2d>,
        Option<&Collider2d>,
        Option<&CharacterController2d>,
        Option<&PhysicsPose2d>,
        Option<&Velocity2d>,
        Option<&CharacterInput2d>,
        Option<&CharacterMotion2d>,
    ), Or<(With<RigidBody2d>, With<CharacterController2d>)>>();
    let mut rows = Vec::new();
    for (entity, id, body, collider, character, pose, velocity, input, motion) in query.iter(world)
    {
        let source = match (body, collider, character) {
            (Some(_), _, Some(_)) => {
                anyhow::bail!(
                    "{}: an entity may have RigidBody2d or CharacterController2d",
                    id.0
                )
            }
            (Some(body), Some(collider), None) => {
                shape::validate_body(body)?;
                Source::Body(*body, collider.clone())
            }
            (Some(_), None, None) => anyhow::bail!("{}: RigidBody2d requires Collider2d", id.0),
            (None, _, Some(character)) => {
                shape::validate_character(character)?;
                Source::Character(*character)
            }
            (None, _, None) => unreachable!("query requires a body or controller"),
        };
        let row = Row {
            entity,
            id: id.0.clone(),
            source,
            pose: pose.copied().unwrap_or_default(),
            velocity: velocity.copied().unwrap_or_default(),
            input: input.copied().unwrap_or_default(),
            grounded: motion.is_some_and(|m| m.grounded),
        };
        shape::validate_state(&row.pose, &row.velocity)?;
        shape::validate_input(&row.input)?;
        rows.push(row);
    }
    rows.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    ensure!(
        rows.windows(2).all(|pair| pair[0].id != pair[1].id),
        "duplicate 2D physics identity"
    );
    Ok(rows)
}

/// Removes stale bindings and creates new ones, both in identity order. Returns the new
/// colliders, which queries cannot see until `refresh_queries`.
fn synchronize(
    world: &mut World,
    state: &mut SolverState,
    rows: &[Row],
) -> Result<Vec<ColliderHandle>> {
    // Validate every collider before mutating the solver.
    for row in rows {
        if let Source::Body(body, collider) = &row.source {
            shape::validate_collider(collider, body)?;
        }
    }
    let find = |id: &str| {
        rows.binary_search_by(|r| r.id.as_str().cmp(id))
            .ok()
            .map(|i| &rows[i])
    };
    let stale: Vec<_> = state
        .bindings
        .iter()
        .filter(|(id, b)| find(id).is_none_or(|row| row.source != b.source))
        .map(|(id, _)| id.to_owned())
        .collect();
    for id in stale {
        if let Some(binding) = state.bindings.remove(&id) {
            state.owners.remove(&binding.collider.into_raw_parts());
            state.world.remove_body(binding.body);
            if let Some(row) = find(&id)
                && !matches!(row.source, Source::Character(_))
            {
                world.entity_mut(row.entity).remove::<CharacterMotion2d>();
            }
        }
    }
    let masks = state.config.layer_masks;
    let mut created = Vec::new();
    for row in rows {
        if state.bindings.contains_key(&row.id) {
            continue;
        }
        let (body, collider) = match &row.source {
            Source::Body(body, collider) => (
                body_builder(body, &row.pose, &row.velocity),
                collider_builder(collider, body, &masks)?,
            ),
            Source::Character(character) => (
                RigidBodyBuilder::kinematic_position_based()
                    .translation(Vector::new(row.pose.position[0], row.pose.position[1]))
                    .lock_rotations(),
                character_collider(character, &masks),
            ),
        };
        let (body, collider) = state.world.insert(body, collider);
        state
            .owners
            .insert(collider.into_raw_parts(), row.id.clone());
        state.bindings.insert(
            row.id.clone(),
            Binding {
                body,
                collider,
                source: row.source.clone(),
            },
        );
        created.push(collider);
    }
    Ok(created)
}

/// Makes new or teleported colliders visible to scene queries before the step. Rapier's
/// collision-only pipeline is not used: it skips island bookkeeping when mixed with steps.
fn refresh_queries(state: &mut SolverState, colliders: &[ColliderHandle]) {
    if colliders.is_empty() {
        return;
    }
    let w = &mut state.world;
    w.bodies
        .propagate_modified_body_positions_to_colliders(&mut w.colliders);
    for &handle in colliders {
        if let Some(collider) = w.colliders.get(handle) {
            let aabb = collider.compute_broad_phase_aabb(&w.integration_parameters, &w.bodies);
            w.broad_phase
                .set_aabb(&w.integration_parameters, handle, aabb);
        }
    }
}

/// Gameplay writes to pose/velocity become teleports, kinematic targets or velocity edits.
/// A character whose pose differs from its body was teleported (for example a respawn).
/// Returns the teleported colliders.
fn apply_writes(state: &mut SolverState, rows: &[Row]) -> Vec<ColliderHandle> {
    let mut moved = Vec::new();
    for row in rows {
        let binding = &state.bindings[&row.id];
        let rb = &mut state.world.bodies[binding.body];
        let pose = Pose::new(
            Vector::new(row.pose.position[0], row.pose.position[1]),
            row.pose.rotation,
        );
        let Source::Body(body, _) = &row.source else {
            if rb.translation() != pose.translation {
                rb.set_position(Pose::from_translation(pose.translation), true);
                moved.push(binding.collider);
            }
            continue;
        };
        if *rb.position() != pose {
            if body.motion == MotionType2d::Kinematic {
                rb.set_next_kinematic_position(pose);
            } else {
                rb.set_position(pose, true);
                moved.push(binding.collider);
            }
        }
        let linear = Vector::new(row.velocity.linear[0], row.velocity.linear[1]);
        if body.motion == MotionType2d::Dynamic && rb.linvel() != linear {
            rb.set_linvel(linear, true);
        }
        if body.motion == MotionType2d::Dynamic && rb.angvel() != row.velocity.angular {
            rb.set_angvel(row.velocity.angular, true);
        }
    }
    moved
}

struct Moved {
    entity: Entity,
    position: Vector,
    velocity: Vector,
    motion: CharacterMotion2d,
}

/// Mirrors ara-physics' CharacterVirtual policy: grounded characters drop vertical speed,
/// jumps launch only from the ground, gravity integrates the rest.
fn move_characters(state: &mut SolverState, rows: &[Row], dt: f32) -> Result<Vec<Moved>> {
    let up = state.config.up();
    let gravity = Vector::new(state.config.gravity[0], state.config.gravity[1]);
    let mut moved = Vec::new();
    for row in rows {
        let Source::Character(c) = &row.source else {
            continue;
        };
        let binding = &state.bindings[&row.id];
        let desired = Vector::new(row.input.desired_velocity[0], row.input.desired_velocity[1]);
        let previous = Vector::new(row.velocity.linear[0], row.velocity.linear[1]);
        let was_grounded = row.grounded;
        let (velocity, jumping) = if let Some(up) = up {
            ensure!(
                desired.dot(up).abs() <= 1e-4 * desired.length().max(1.),
                "{}: desired character velocity must be perpendicular to gravity",
                row.id
            );
            let mut vertical = if was_grounded { 0. } else { previous.dot(up) };
            let jumping = was_grounded && row.input.jump_speed > 0.;
            if jumping {
                vertical += row.input.jump_speed;
            }
            (desired + up * vertical + gravity * dt, jumping)
        } else {
            ensure!(row.input.jump_speed == 0., "jumps require gravity");
            (desired, false)
        };
        let controller = KinematicCharacterController {
            up: up.unwrap_or(Vector::Y),
            offset: CharacterLength::Absolute(c.padding),
            max_slope_climb_angle: c.max_slope_deg.to_radians(),
            min_slope_slide_angle: c.max_slope_deg.to_radians(),
            autostep: (c.step_height > 0.).then_some(CharacterAutostep {
                max_height: CharacterLength::Absolute(c.step_height),
                min_width: CharacterLength::Absolute(c.radius),
                include_dynamic_bodies: false,
            }),
            snap_to_ground: (c.stick_down > 0. && !jumping && up.is_some())
                .then_some(CharacterLength::Absolute(c.stick_down)),
            ..Default::default()
        };
        let collider = &state.world.colliders[binding.collider];
        let queries = state.world.query_pipeline_with_filter(
            QueryFilter::default()
                .exclude_rigid_body(binding.body)
                .exclude_sensors()
                .groups(shape::groups(c.layer, &state.config.layer_masks)),
        );
        let mut ground_normal = Vector::ZERO;
        let start = *collider.position();
        let movement = controller.move_shape(
            dt,
            &queries,
            collider.shape(),
            &start,
            velocity * dt,
            |hit| {
                // normal1 is in the upright capsule's frame and points at the obstacle.
                let normal = -hit.hit.normal1;
                if up.is_some_and(|up| normal.dot(up) > ground_normal.dot(up)) {
                    ground_normal = normal;
                }
            },
        );
        let position = start.translation + movement.translation;
        let mut actual = movement.translation / dt;
        if let Some(up) = up
            && movement.grounded
            && actual.dot(up) < 0.
        {
            actual -= up * actual.dot(up);
        }
        state.world.bodies[binding.body]
            .set_next_kinematic_position(Pose::from_translation(position));
        moved.push(Moved {
            entity: row.entity,
            position,
            velocity: actual,
            motion: CharacterMotion2d {
                grounded: movement.grounded,
                ground_normal: if movement.grounded {
                    let n = if ground_normal == Vector::ZERO {
                        up.unwrap_or(Vector::ZERO)
                    } else {
                        ground_normal
                    };
                    [n.x, n.y]
                } else {
                    [0.; 2]
                },
            },
        });
    }
    Ok(moved)
}

fn run(world: &mut World, state: &mut SolverState) -> Result<()> {
    let dt = 1. / state.config.tick_hz as f32;
    if let Some(context) = world.get_resource::<SimContext>() {
        ensure!(
            (context.dt() - f64::from(dt)).abs() < 1e-6,
            "2D physics requires its configured fixed tick"
        );
    }
    let rows = rows(world)?;
    let mut touched = synchronize(world, state, &rows)?;
    touched.extend(apply_writes(state, &rows));
    refresh_queries(state, &touched);
    let characters = move_characters(state, &rows, dt)?;
    state.world.step();
    // Static bodies never move on their own; skipping them keeps tile worlds cheap.
    for row in &rows {
        if let Source::Body(body, _) = &row.source
            && body.motion != MotionType2d::Static
        {
            let rb = &state.world.bodies[state.bindings[&row.id].body];
            let (p, v) = (rb.translation(), rb.linvel());
            world.entity_mut(row.entity).insert((
                PhysicsPose2d {
                    position: [p.x, p.y],
                    rotation: rb.rotation().angle(),
                },
                Velocity2d {
                    linear: [v.x, v.y],
                    angular: rb.angvel(),
                },
            ));
        }
    }
    for c in characters {
        world.entity_mut(c.entity).insert((
            PhysicsPose2d {
                position: [c.position.x, c.position.y],
                rotation: 0.,
            },
            Velocity2d {
                linear: [c.velocity.x, c.velocity.y],
                angular: 0.,
            },
            c.motion,
        ));
    }
    Ok(())
}
