//! Fixed-tick crowd synchronization; no authoritative state survives outside ECS.
use crate::{
    Authority, NavAgent, NavAgentState, NavError, NavMesh, NavPosition, NavStatus, NavWorld,
    Traversal, distance, finite,
};
use bevy_ecs::prelude::{Entity, World};
use std::collections::BTreeSet;

/// Publishes permitted completed bakes and advances server agents by one fixed tick.
/// Stable component IDs define order; ECS allocation/iteration order has no effect.
pub fn tick(world: &mut World) {
    if !world.contains_resource::<NavWorld>() {
        return;
    }
    world.resource_scope(
        |world, mut nav: bevy_ecs::change_detection::Mut<NavWorld>| {
            nav.errors.clear();
            if nav.config.allow_mesh_swap {
                let pending = std::mem::take(&mut nav.pending);
                for (profile, mesh) in pending {
                    nav.meshes.insert(profile, mesh);
                }
            }
            if nav.config.authority == Authority::Client {
                return;
            }
            nav.agents.clear();
            nav.agents.extend(
                world
                    .query::<(Entity, &NavAgentState)>()
                    .iter(world)
                    .map(|(e, s)| (s.id, e)),
            );
            nav.agents.sort_unstable_by_key(|&(id, _)| id);
            if nav.agents.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                nav.errors.push((
                    0,
                    NavError::new("duplicateAgentId", "stable navigation IDs must be unique"),
                ));
                return;
            }
            let profiles: BTreeSet<u32> = nav
                .agents
                .iter()
                .filter_map(|&(_, e)| world.get::<NavAgent>(e).map(|a| a.profile))
                .collect();
            let dt = nav.config.fixed_dt;
            for profile in profiles {
                // Destructure to borrow disjoint resource fields; no per-tick mesh clones.
                let NavWorld {
                    meshes,
                    agents,
                    scratch,
                    entities,
                    errors,
                    ..
                } = &mut *nav;
                scratch.clear();
                entities.clear();
                let Some(mesh) = meshes.get(&profile) else {
                    for &(id, e) in agents.iter() {
                        if world
                            .get::<NavAgent>(e)
                            .is_some_and(|a| a.profile == profile)
                        {
                            errors.push((
                                id,
                                NavError::new("missingMesh", "agent profile has no published mesh"),
                            ));
                            if let Some(mut state) = world.get_mut::<NavAgentState>(e) {
                                state.status = NavStatus::Unreachable;
                                state.velocity = [0.0; 3];
                            }
                        }
                    }
                    continue;
                };
                let mut query = match mesh.query() {
                    Ok(q) => q,
                    Err(e) => {
                        errors.push((0, e));
                        continue;
                    }
                };
                let mut query_state =
                    world.query::<(&NavAgent, &mut NavAgentState, &mut NavPosition)>();
                for &(id, entity) in agents.iter() {
                    let Ok((agent, mut state, mut position)) = query_state.get_mut(world, entity)
                    else {
                        continue;
                    };
                    if agent.profile != profile {
                        continue;
                    }
                    if let Err(error) = validate_agent(agent, &state, position.0, mesh) {
                        state.status = NavStatus::Unreachable;
                        state.velocity = [0.0; 3];
                        errors.push((id, error));
                        continue;
                    }
                    let mut permitted = state.filter;
                    permitted.include &= agent.area_mask;
                    if state.traversal.is_none()
                        && !query
                            .nearest(position.0, &permitted)
                            .is_ok_and(|p| p.is_some())
                    {
                        state.status = NavStatus::Unreachable;
                        state.velocity = [0.0; 3];
                        errors.push((
                            id,
                            NavError::new("agentOutside", "agent has no permitted nearby polygon"),
                        ));
                        continue;
                    }
                    if let Some(mut traversal) = state.traversal {
                        traversal.elapsed = (traversal.elapsed + dt).min(traversal.duration);
                        let f = (traversal.elapsed / traversal.duration).min(1.0);
                        position.0 = std::array::from_fn(|i| {
                            traversal.start[i] + (traversal.end[i] - traversal.start[i]) * f
                        });
                        state.velocity = std::array::from_fn(|i| {
                            (traversal.end[i] - traversal.start[i]) / traversal.duration
                        });
                        if f >= 1.0 {
                            state.traversal = None;
                            state.corner += 2;
                        } else {
                            state.traversal = Some(traversal);
                        }
                        continue;
                    }
                    let mut desired = [0.0; 3];
                    if let Some(target) = state.target {
                        if distance(position.0, target) <= agent.stopping_distance {
                            state.status = NavStatus::Arrived;
                            state.velocity = [0.0; 3];
                        } else {
                            if state.status == NavStatus::Arrived {
                                state.status = NavStatus::Moving;
                            }
                            let mut filter = state.filter;
                            filter.include &= agent.area_mask;
                            if state.path.is_none()
                                || state.mesh != Some(mesh.content_hash())
                                || state.planned_filter != Some(filter)
                            {
                                state.planned_filter = Some(filter);
                                match query.path(position.0, target, &filter) {
                                    Ok(path) if path.complete => {
                                        state.path = Some(path);
                                        state.mesh = Some(mesh.content_hash());
                                        state.corner = 1;
                                        state.status = NavStatus::Moving;
                                    }
                                    Ok(path) => {
                                        state.path = Some(path);
                                        state.mesh = Some(mesh.content_hash());
                                        state.status = NavStatus::Unreachable;
                                        state.velocity = [0.0; 3];
                                    }
                                    Err(e) => {
                                        state.status = NavStatus::Unreachable;
                                        state.velocity = [0.0; 3];
                                        errors.push((id, e));
                                    }
                                }
                            }
                            if state.status == NavStatus::Moving {
                                // Avoidance keeps a radius away from walls. Advance
                                // intermediate corners before demanding that every
                                // agent occupy the same exact funnel vertex.
                                let tolerance = agent
                                    .stopping_distance
                                    .max(agent.max_speed * dt)
                                    .max(agent.radius * 2.0);
                                while let Some((current, next)) = state
                                    .path
                                    .as_ref()
                                    .and_then(|p| {
                                        p.corners
                                            .get(state.corner)
                                            .zip(p.corners.get(state.corner + 1))
                                    })
                                    .map(|(a, b)| (*a, *b))
                                {
                                    if current.off_mesh
                                        || distance(position.0, current.position)
                                            > agent.radius * 6.0
                                        || !query
                                            .visible(position.0, next.position, &permitted)
                                            .unwrap_or(false)
                                    {
                                        break;
                                    }
                                    state.corner += 1;
                                }
                                while let Some(corner) = state
                                    .path
                                    .as_ref()
                                    .and_then(|p| p.corners.get(state.corner))
                                    .copied()
                                {
                                    let terminal = state
                                        .path
                                        .as_ref()
                                        .is_some_and(|p| state.corner + 1 == p.corners.len());
                                    let reach = if terminal {
                                        agent.stopping_distance
                                    } else {
                                        tolerance
                                    };
                                    if distance(position.0, corner.position) > reach
                                        || corner.off_mesh
                                    {
                                        break;
                                    }
                                    if let Some(next) = state
                                        .path
                                        .as_ref()
                                        .and_then(|p| p.corners.get(state.corner + 1))
                                        && !query
                                            .visible(position.0, next.position, &permitted)
                                            .unwrap_or(false)
                                    {
                                        break;
                                    }
                                    state.corner += 1;
                                }
                                if let Some(corner) = state
                                    .path
                                    .as_ref()
                                    .and_then(|p| p.corners.get(state.corner))
                                    .copied()
                                {
                                    if corner.off_mesh
                                        && agent.max_speed > 0.0
                                        && distance(position.0, corner.position) <= tolerance
                                        && let Some(end) = state
                                            .path
                                            .as_ref()
                                            .and_then(|p| p.corners.get(state.corner + 1))
                                            .map(|c| c.position)
                                    {
                                        state.traversal = Some(Traversal {
                                            start: corner.position,
                                            end,
                                            elapsed: 0.0,
                                            duration: (distance(corner.position, end)
                                                / agent.max_speed.max(0.01))
                                            .max(dt),
                                        });
                                        state.velocity = [0.0; 3];
                                        continue;
                                    }
                                    let mut dx = corner.position[0] - position.0[0];
                                    let mut dz = corner.position[2] - position.0[2];
                                    let distance_to_corner = (dx * dx + dz * dz).sqrt();
                                    // Anticipate the turn from outside the corner,
                                    // following DetourCrowd's zlib-licensed rule.
                                    // See ara-nav-sys/VENDOR.md for the source notice.
                                    if !corner.off_mesh
                                        && let Some(next) = state
                                            .path
                                            .as_ref()
                                            .and_then(|p| p.corners.get(state.corner + 1))
                                    {
                                        let nx = next.position[0] - position.0[0];
                                        let nz = next.position[2] - position.0[2];
                                        let len = (nx * nx + nz * nz).sqrt();
                                        if len > 0.001 {
                                            dx -= nx / len * distance_to_corner * 0.5;
                                            dz -= nz / len * distance_to_corner * 0.5;
                                        }
                                    }
                                    let length = (dx * dx + dz * dz).sqrt();
                                    if length > 0.0001 {
                                        let speed = agent.max_speed.min(length / dt);
                                        desired = [dx / length * speed, 0.0, dz / length * speed];
                                    }
                                } else {
                                    // Funnel may project a distant off-mesh goal. Never
                                    // report arrival unless the original target is near.
                                    state.status = NavStatus::Unreachable;
                                }
                            }
                        }
                    }
                    scratch.push(ara_nav_sys::Agent {
                        position: position.0,
                        velocity: state.velocity,
                        desired,
                        radius: agent.radius,
                        height: agent.height,
                        speed: agent.max_speed,
                        acceleration: agent.acceleration,
                        include: state.filter.include & agent.area_mask,
                        exclude: state.filter.exclude,
                    });
                    entities.push(entity);
                }
                if scratch.len() > 100000 {
                    errors.push((0, NavError::new("capacity", "crowd exceeds 100000 agents")));
                    continue;
                }
                if let Err(error) = mesh.native().crowd_step(scratch, dt) {
                    errors.push((0, error));
                    continue;
                }
                for (&entity, result) in entities.iter().zip(scratch.iter()) {
                    if let Ok((_, mut state, mut position)) = query_state.get_mut(world, entity) {
                        position.0 = result.position;
                        state.velocity = result.velocity;
                    }
                }
            }
        },
    );
}
fn validate_agent(
    a: &NavAgent,
    s: &NavAgentState,
    p: [f32; 3],
    mesh: &NavMesh,
) -> Result<(), NavError> {
    let profile = mesh.profile();
    if !finite(p)
        || p.iter().any(|v| v.abs() > 1_000_000.0)
        || !finite(s.velocity)
        || s.target.is_some_and(|t| !finite(t))
        || !(0.001..=profile.radius).contains(&a.radius)
        || !(0.01..=profile.height).contains(&a.height)
        || !(0.0..=1000.0).contains(&a.max_speed)
        || !(0.0..=10000.0).contains(&a.acceleration)
        || !(0.001..=10.0).contains(&a.stopping_distance)
        || s.filter.include & a.area_mask & !s.filter.exclude == 0
        || s.traversal.is_some_and(|t| {
            !finite(t.start)
                || !finite(t.end)
                || !t.elapsed.is_finite()
                || !t.duration.is_finite()
                || t.duration <= 0.0
                || t.elapsed < 0.0
                || t.elapsed > t.duration
        })
    {
        return Err(NavError::new(
            "invalidAgent",
            "invalid state or dimensions larger than baked clearance",
        ));
    }
    s.filter.native()?;
    Ok(())
}
