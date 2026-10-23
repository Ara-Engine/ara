//! Large-crowd tick: budgeted replans in stable-ID order, parallel path following
//! over ECS table slices, then one deterministic ORCA solve for every profile.
use crate::{
    NavAgent, NavAgentState, NavError, NavIssue, NavPosition, NavStatus, NavWorld, PathRequest,
    QueryFilter, finite,
    follow::{self, Follow},
};
use bevy_ecs::{prelude::World, query::QueryState};
use rayon::prelude::*;

pub(crate) type AgentQuery = QueryState<(
    &'static NavAgent,
    &'static mut NavAgentState,
    &'static mut NavPosition,
)>;

type View<'a> = (
    &'a [NavAgent],
    &'a mut [NavAgentState],
    &'a mut [NavPosition],
);

/// A replan candidate: stable ID first so budget order never depends on ECS order.
#[derive(Clone, Copy)]
pub(crate) struct Want {
    validate: bool,
    has_path: bool,
    profile: u32,
    avoid: Option<u32>,
    id: u64,
    table: u32,
    row: u32,
}

pub(crate) fn step(world: &mut World, nav: &mut NavWorld) {
    let mut query = nav.agent_query.take().unwrap_or_else(|| world.query());
    run(world, nav, &mut query);
    nav.agent_query = Some(query);
}

fn run(world: &mut World, nav: &mut NavWorld, query: &mut AgentQuery) {
    // Every stable ID must be unique, including states missing other components.
    nav.ids.clear();
    nav.ids
        .extend(world.query::<&NavAgentState>().iter(world).map(|s| s.id));
    nav.ids.sort_unstable();
    if nav.ids.windows(2).any(|pair| pair[0] == pair[1]) {
        nav.errors.push((
            0,
            NavError::new("duplicateAgentId", "stable navigation IDs must be unique"),
        ));
        return;
    }
    let Ok(iter) = query.contiguous_iter_mut(world) else {
        nav.errors.push((
            0,
            NavError::new("storage", "navigation agents must use table storage"),
        ));
        return;
    };
    let mut tables: Vec<_> = iter.collect();
    let mut views: Vec<View<'_>> = tables
        .iter_mut()
        .map(|(agents, states, positions)| {
            states.mark_all_as_changed();
            positions.mark_all_as_changed();
            (
                *agents,
                states.bypass_change_detection(),
                positions.bypass_change_detection(),
            )
        })
        .collect();

    plan(nav, &mut views);

    let cfg = Follow {
        dt: nav.config.fixed_dt,
        link_wait_ticks: nav.config.link_wait_ticks,
        stall_ticks: nav.config.stall_ticks,
    };
    let meshes = &nav.meshes;
    let gates = &nav.gates;
    for (agents, states, positions) in &mut views {
        agents
            .par_iter()
            .zip(states.par_iter_mut())
            .zip(positions.par_iter_mut())
            .with_min_len(256)
            .for_each(|((agent, state), position)| {
                let Some(mesh) = meshes.get(&agent.profile) else {
                    state.status = NavStatus::Unreachable;
                    state.issue = Some(NavIssue::NoMesh);
                    state.velocity = [0.0; 3];
                    state.desired = [0.0; 3];
                    return;
                };
                if !valid(agent, state, position.0, mesh.profile()) {
                    state.status = NavStatus::Unreachable;
                    state.issue = Some(NavIssue::InvalidAgent);
                    state.velocity = [0.0; 3];
                    state.desired = [0.0; 3];
                    return;
                }
                if matches!(state.issue, Some(NavIssue::NoMesh | NavIssue::InvalidAgent)) {
                    state.issue = None;
                }
                if follow::advance_traversal(state, &mut position.0, cfg.dt) {
                    return;
                }
                state.desired = follow::steer(agent, state, position.0, mesh.links(), gates, cfg);
            });
    }

    // Gather avoidance input in iteration order; results do not depend on it.
    let profiles: Vec<u32> = nav.meshes.keys().copied().collect();
    nav.crowd_agents.clear();
    nav.slots.clear();
    nav.waiting.clear();
    for (t, (agents, states, positions)) in views.iter().enumerate() {
        for (r, ((agent, state), position)) in agents
            .iter()
            .zip(states.iter())
            .zip(positions.iter())
            .enumerate()
        {
            match state.issue {
                Some(NavIssue::NoMesh) => nav.errors.push((
                    state.id,
                    NavError::new("missingMesh", "agent profile has no published mesh"),
                )),
                Some(NavIssue::StartOffMesh) => nav.errors.push((
                    state.id,
                    NavError::new("agentOutside", "agent has no permitted nearby polygon"),
                )),
                Some(NavIssue::InvalidAgent) => nav.errors.push((
                    state.id,
                    NavError::new(
                        "invalidAgent",
                        "invalid state or dimensions larger than baked clearance",
                    ),
                )),
                _ => {}
            }
            if let Some((link, _)) = state.waiting {
                *nav.waiting.entry(link).or_default() += 1;
            }
            let Ok(wall_index) = profiles.binary_search(&agent.profile) else {
                continue;
            };
            if state.traversal.is_some() || matches!(state.issue, Some(NavIssue::InvalidAgent)) {
                continue;
            }
            nav.slots.push((t as u32, r as u32));
            nav.crowd_agents.push(ara_crowd::Agent {
                id: state.id,
                position: position.0,
                velocity: state.velocity,
                preferred: state.desired,
                radius: agent.radius,
                height: agent.height,
                max_speed: agent.max_speed,
                max_acceleration: agent.acceleration,
                layers: 1,
                walls: wall_index as u16,
                obstacle: false,
            });
        }
    }
    let real = nav.crowd_agents.len();
    nav.crowd_agents.extend_from_slice(&nav.obstacle_discs);
    let walls: Vec<&ara_crowd::Walls> = nav.meshes.values().map(|m| m.walls()).collect();
    let dt = nav.config.fixed_dt;
    match nav.crowd.step(&mut nav.crowd_agents, &walls, dt) {
        Ok(_) => {
            for (&(t, r), result) in nav.slots.iter().zip(&nav.crowd_agents[..real]) {
                let (_, states, positions) = &mut views[t as usize];
                positions[r as usize].0 = result.position;
                states[r as usize].velocity = result.velocity;
            }
        }
        Err(e) => nav.errors.push((0, NavError::new("crowd", e.0))),
    }
}

/// Effective filter: the agent type's costs/mask times the agent's own filter.
pub(crate) fn effective_filter(
    agent: &NavAgent,
    state: &NavAgentState,
    types: &std::collections::BTreeMap<u32, QueryFilter>,
) -> QueryFilter {
    let mut filter = state.filter;
    if let Some(t) = types.get(&agent.profile) {
        for (cost, scale) in filter.costs.iter_mut().zip(t.costs) {
            *cost = (*cost * scale).min(1_000_000.0);
        }
        filter.include &= t.include;
        filter.exclude |= t.exclude;
    }
    filter.include &= agent.area_mask;
    filter
}

/// Replans within `NavConfig::replan_budget`: agents without a corridor first,
/// then stale ones (mesh swapped, filter changed), each class by stable ID.
fn plan(nav: &mut NavWorld, views: &mut [View<'_>]) {
    nav.wants.clear();
    for (t, (agents, states, _)) in views.iter().enumerate() {
        for (r, (agent, state)) in agents.iter().zip(states.iter()).enumerate() {
            if state.traversal.is_some() || state.waiting.is_some() {
                continue;
            }
            let Some(mesh) = nav.meshes.get(&agent.profile) else {
                continue;
            };
            if state.target.is_none() {
                // Idle agents are projected once per mesh revision (off-mesh check).
                if state.mesh != Some(mesh.content_hash()) {
                    nav.wants.push(Want {
                        validate: true,
                        has_path: false,
                        profile: agent.profile,
                        avoid: None,
                        id: state.id,
                        table: t as u32,
                        row: r as u32,
                    });
                }
                continue;
            }
            let filter = effective_filter(agent, state, &nav.types);
            let stale =
                state.mesh != Some(mesh.content_hash()) || state.planned_filter != Some(filter);
            let missing = state.path.is_none() && state.status != NavStatus::Unreachable;
            if stale || missing {
                nav.wants.push(Want {
                    validate: false,
                    has_path: state.path.is_some(),
                    profile: agent.profile,
                    avoid: state.avoid_link,
                    id: state.id,
                    table: t as u32,
                    row: r as u32,
                });
            }
        }
    }
    nav.wants
        .sort_unstable_by_key(|w| (w.validate, w.has_path, w.id));
    nav.wants.truncate(nav.config.replan_budget);
    validate_idle(nav, views);
    nav.wants.retain(|w| !w.validate);
    nav.wants
        .sort_unstable_by_key(|w| (w.profile, w.avoid, w.id));
    let mut start = 0;
    while start < nav.wants.len() {
        let key = (nav.wants[start].profile, nav.wants[start].avoid);
        let end = start
            + nav.wants[start..]
                .iter()
                .take_while(|w| (w.profile, w.avoid) == key)
                .count();
        let Some(mesh) = nav.meshes.get(&key.0) else {
            start = end;
            continue;
        };
        let mut blocked = nav.gates.blocked.clone();
        if let Some(link) = key.1 {
            blocked.push(link);
        }
        nav.requests.clear();
        nav.filters.clear();
        for w in &nav.wants[start..end] {
            let (agents, states, positions) = &views[w.table as usize];
            let (agent, state) = (&agents[w.row as usize], &states[w.row as usize]);
            let filter = effective_filter(agent, state, &nav.types);
            nav.filters.push(filter);
            nav.requests.push(PathRequest {
                start: positions[w.row as usize].0,
                end: state.target.unwrap_or(positions[w.row as usize].0),
                filter,
            });
        }
        let results = mesh.paths_blocked(&nav.requests, &blocked);
        let hash = mesh.content_hash();
        for ((w, result), filter) in nav.wants[start..end].iter().zip(results).zip(&nav.filters) {
            let state = &mut views[w.table as usize].1[w.row as usize];
            state.mesh = Some(hash);
            state.planned_filter = Some(*filter);
            state.corner = 1;
            match result {
                Ok(path) if path.complete => {
                    state.path = Some(path);
                    state.status = NavStatus::Moving;
                    if !matches!(state.issue, Some(NavIssue::LinkTimeout { .. })) {
                        state.issue = None;
                    }
                }
                Ok(path) => {
                    state.issue = Some(NavIssue::NoPath {
                        reachable: path.reachable,
                    });
                    state.path = Some(path);
                    state.status = NavStatus::Unreachable;
                    state.velocity = [0.0; 3];
                }
                Err(_) => {
                    state.issue = Some(NavIssue::StartOffMesh);
                    state.path = None;
                    state.status = NavStatus::Unreachable;
                    state.velocity = [0.0; 3];
                }
            }
        }
        start = end;
    }
}

fn valid(
    agent: &NavAgent,
    state: &NavAgentState,
    p: [f32; 3],
    profile: crate::AgentProfile,
) -> bool {
    finite(p)
        && p.iter().all(|v| v.abs() <= 1_000_000.0)
        && finite(state.velocity)
        && state.target.is_none_or(finite)
        && (0.001..=profile.radius).contains(&agent.radius)
        && (0.01..=profile.height).contains(&agent.height)
        && (0.0..=1000.0).contains(&agent.max_speed)
        && (0.0..=10000.0).contains(&agent.acceleration)
        && (0.001..=10.0).contains(&agent.stopping_distance)
        && state.filter.validate().is_ok()
        && state.filter.include & agent.area_mask & !state.filter.exclude != 0
}

/// Projects idle agents queued for validation; off-mesh ones become Unreachable.
fn validate_idle(nav: &mut NavWorld, views: &mut [View<'_>]) {
    let mut queries: std::collections::BTreeMap<u32, crate::NavQuery<'_>> = Default::default();
    for w in nav.wants.iter().filter(|w| w.validate) {
        let Some(mesh) = nav.meshes.get(&w.profile) else {
            continue;
        };
        let query = match queries.entry(w.profile) {
            std::collections::btree_map::Entry::Occupied(q) => q.into_mut(),
            std::collections::btree_map::Entry::Vacant(slot) => match mesh.query() {
                Ok(q) => slot.insert(q),
                Err(e) => {
                    nav.errors.push((0, e));
                    continue;
                }
            },
        };
        let (agents, states, positions) = &mut views[w.table as usize];
        let (agent, state) = (&agents[w.row as usize], &mut states[w.row as usize]);
        let filter = effective_filter(agent, state, &nav.types);
        let on_mesh = query
            .nearest(positions[w.row as usize].0, &filter)
            .is_ok_and(|p| p.is_some());
        state.mesh = Some(mesh.content_hash());
        state.planned_filter = Some(filter);
        if on_mesh {
            if state.issue == Some(NavIssue::StartOffMesh) {
                state.issue = None;
                state.status = NavStatus::Idle;
            }
        } else {
            state.issue = Some(NavIssue::StartOffMesh);
            state.status = NavStatus::Unreachable;
            state.velocity = [0.0; 3];
        }
    }
}
