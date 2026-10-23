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
    let mut ids = nav.id_query.take().unwrap_or_else(|| world.query());
    nav.ids.extend(ids.iter(world).map(|s| s.id));
    nav.id_query = Some(ids);
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

    // Errors and link waiting counts (cheap serial scan of two fields).
    nav.waiting.clear();
    for (_, states, _) in views.iter() {
        for state in states.iter() {
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
        }
    }

    // Avoidance input: one slot per agent in iteration order (results do not
    // depend on it). Traversing, invalid or meshless agents become inert slots:
    // no layers (nothing avoids them) and fixed (they do not move).
    let profiles: Vec<u32> = nav.meshes.keys().copied().collect();
    let total: usize = views.iter().map(|v| v.0.len()).sum();
    nav.crowd_agents.clear();
    nav.crowd_agents.resize(total, ara_crowd::Agent::default());
    let mut rest = nav.crowd_agents.as_mut_slice();
    for (agents, states, positions) in views.iter() {
        let (slots, tail) = rest.split_at_mut(agents.len());
        rest = tail;
        slots
            .par_iter_mut()
            .zip(agents.par_iter())
            .zip(states.par_iter())
            .zip(positions.par_iter())
            .with_min_len(512)
            .for_each(|(((slot, agent), state), position)| {
                let wall_index = profiles.binary_search(&agent.profile).ok();
                let inert = wall_index.is_none()
                    || state.traversal.is_some()
                    || matches!(state.issue, Some(NavIssue::InvalidAgent | NavIssue::NoMesh));
                *slot = ara_crowd::Agent {
                    id: state.id,
                    position: if inert { [0.0; 3] } else { position.0 },
                    velocity: if inert { [0.0; 3] } else { state.velocity },
                    preferred: if inert { [0.0; 3] } else { state.desired },
                    radius: agent.radius.clamp(0.001, 10.0),
                    height: agent.height.max(0.01),
                    max_speed: if inert { 0.0 } else { agent.max_speed },
                    max_acceleration: agent.acceleration.max(0.0),
                    layers: if inert { 0 } else { 1 },
                    walls: wall_index.map_or(ara_crowd::NO_WALLS, |i| i as u16),
                    obstacle: inert,
                };
            });
    }
    nav.crowd_agents.extend_from_slice(&nav.obstacle_discs);
    let walls: Vec<&ara_crowd::Walls> = nav.meshes.values().map(|m| m.walls()).collect();
    let dt = nav.config.fixed_dt;
    if let Err(e) = nav.crowd.step(&mut nav.crowd_agents, &walls, dt) {
        nav.errors.push((0, NavError::new("crowd", e.0)));
        return;
    }
    let mut results = &nav.crowd_agents[..total];
    for (_, states, positions) in views.iter_mut() {
        let (chunk, tail) = results.split_at(states.len());
        results = tail;
        chunk
            .par_iter()
            .zip(states.par_iter_mut())
            .zip(positions.par_iter_mut())
            .with_min_len(512)
            .for_each(|((result, state), position)| {
                if !result.obstacle {
                    position.0 = result.position;
                    state.velocity = result.velocity;
                }
            });
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

/// Whether an agent needs work this tick: `(validate_only, has_path)`. Idle
/// agents are projected once per mesh revision (off-mesh check); moving agents
/// replan when the corridor is missing, the mesh swapped or the filter changed.
fn candidate(
    agent: &NavAgent,
    state: &NavAgentState,
    meshes: &std::collections::BTreeMap<u32, crate::NavMesh>,
    types: &std::collections::BTreeMap<u32, QueryFilter>,
) -> Option<(bool, bool)> {
    if state.traversal.is_some() || state.waiting.is_some() {
        return None;
    }
    let hash = meshes.get(&agent.profile)?.content_hash();
    if state.target.is_none() {
        return (state.mesh != Some(hash)).then_some((true, false));
    }
    let stale = state.mesh != Some(hash)
        || state.planned_filter != Some(effective_filter(agent, state, types));
    let missing = state.path.is_none() && state.status != NavStatus::Unreachable;
    (stale || missing).then_some((false, state.path.is_some()))
}

/// Replans within `NavConfig::replan_budget`: agents without a corridor first,
/// then stale ones (mesh swapped, filter changed), each class by stable ID.
fn plan(nav: &mut NavWorld, views: &mut [View<'_>]) {
    nav.wants.clear();
    let (meshes, types) = (&nav.meshes, &nav.types);
    for (t, (agents, states, _)) in views.iter().enumerate() {
        // Parallel scan; `par_extend` keeps row order, and the result is sorted below.
        nav.wants.par_extend(
            agents
                .par_iter()
                .zip(states.par_iter())
                .enumerate()
                .with_min_len(1024)
                .filter_map(|(r, (agent, state))| {
                    candidate(agent, state, meshes, types).map(|(validate, has_path)| Want {
                        validate,
                        has_path,
                        profile: agent.profile,
                        avoid: state.avoid_link,
                        id: state.id,
                        table: t as u32,
                        row: r as u32,
                    })
                }),
        );
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
                end: leg_goal(state).unwrap_or(state.target.unwrap_or(positions[w.row as usize].0)),
                filter,
            });
        }
        let results = mesh.paths_blocked(&nav.requests, &blocked);
        let hash = mesh.content_hash();
        let graph = nav.graphs.get(&key.0).cloned();
        let mut probe = None;
        for (((w, result), filter), request) in nav.wants[start..end]
            .iter()
            .zip(results)
            .zip(&nav.filters)
            .zip(&nav.requests)
        {
            let (_, states, positions) = &mut views[w.table as usize];
            let state = &mut states[w.row as usize];
            let position = positions[w.row as usize].0;
            state.mesh = Some(hash);
            state.planned_filter = Some(*filter);
            state.corner = 1;
            let on_leg = leg_goal(state).is_some();
            // Detour fails outright when the end has no polygon; with the start on the
            // mesh that is an empty partial corridor (goal outside the resident mesh).
            let result = match result {
                Err(_) if !outside(mesh, &mut probe, position, filter) => Ok(crate::NavPath {
                    corners: Vec::new(),
                    complete: false,
                    reachable: position,
                    truncated: false,
                }),
                other => other,
            };
            match result {
                Ok(path) if path.complete => {
                    if !on_leg {
                        state.route = None; // The goal itself is resident and reachable.
                    }
                    state.path = Some(path);
                    state.status = NavStatus::Moving;
                    if !matches!(state.issue, Some(NavIssue::LinkTimeout { .. })) {
                        state.issue = None;
                    }
                }
                Ok(path) if on_leg => {
                    // Next portal not resident yet: walk toward the boundary and wait.
                    state.path = Some(path);
                    state.status = NavStatus::Moving;
                    state.issue = Some(NavIssue::OutsideResident);
                }
                Ok(path) if graph.is_some() && outside(mesh, &mut probe, request.end, filter) => {
                    let Some(graph) = graph.as_ref() else {
                        continue;
                    };
                    match graph.route(position, request.end, &mut nav.route_scratch) {
                        Ok((Some(route), _)) => {
                            state.route = Some(route);
                            state.path = Some(path);
                            state.planned_filter = None; // Replan toward the first leg.
                            state.status = NavStatus::Moving;
                            state.issue = Some(NavIssue::OutsideResident);
                        }
                        _ => {
                            state.issue = Some(NavIssue::NoPath {
                                reachable: path.reachable,
                            });
                            state.path = Some(path);
                            state.status = NavStatus::Unreachable;
                            state.velocity = [0.0; 3];
                        }
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

/// Intermediate route waypoint the agent is heading for, if it follows a route
/// and has not reached its final leg.
pub(crate) fn leg_goal(state: &NavAgentState) -> Option<[f32; 3]> {
    let route = state.route.as_ref()?;
    (route.next + 1 < route.waypoints.len()).then(|| route.waypoints[route.next])
}

/// Whether `point` has no permitted polygon nearby on the resident `mesh`.
fn outside<'m>(
    mesh: &'m crate::NavMesh,
    probe: &mut Option<crate::NavQuery<'m>>,
    point: [f32; 3],
    filter: &QueryFilter,
) -> bool {
    if probe.is_none() {
        *probe = mesh.query().ok();
    }
    probe
        .as_mut()
        .is_some_and(|q| q.nearest(point, filter).is_ok_and(|p| p.is_none()))
}
