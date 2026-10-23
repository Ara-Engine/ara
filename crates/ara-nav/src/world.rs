//! Stable-order fixed stepping with authoritative state exclusively in components.
use crate::{
    LinkKind, NavAgent, NavAgentState, NavAgentType, NavError, NavLinkState, NavMesh, NavPosition,
    QueryFilter, finite, follow::LinkGates, orca_step::AgentQuery,
};
use bevy_ecs::{
    prelude::{Entity, Resource, World},
    schedule::{IntoScheduleConfigs, Schedule, SystemSet},
};
use std::collections::BTreeMap;

/// Fixed schedule set: order after transform synchronization and before physics extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SystemSet)]
pub struct NavigationTick;

/// World role; replicated clients never run authoritative navigation simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// Local, listen-server or dedicated-server simulation.
    Server,
    /// Apply replicated positions/state; never simulate AI locally.
    Client,
}

/// Local avoidance solver. Chosen by measurement (see RESULTS.md): ORCA scales to
/// 10k agents in parallel; DetourCrowd adds per-agent surface projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrowdSolver {
    /// Deterministic parallel ORCA (`ara-crowd`) against navmesh walls; one solve
    /// across all profiles. Path following is pure Rust and budgeted.
    Orca,
    /// DetourCrowd per profile (single-threaded native scratch each tick).
    Detour,
}

/// Fixed navigation tick and non-disruption publication policy.
#[derive(Debug, Clone, Copy)]
pub struct NavConfig {
    /// Fixed seconds per tick; set once, never derive from the wall clock.
    pub fixed_dt: f32,
    /// Whether this world runs authoritative AI.
    pub authority: Authority,
    /// Whether ready agent bakes may swap at this boundary (Play/approval gate).
    pub allow_mesh_swap: bool,
    /// Local avoidance solver.
    pub crowd: CrowdSolver,
    /// ORCA tuning (ignored by the Detour solver).
    pub crowd_params: ara_crowd::Params,
    /// Path queries per tick (ORCA solver); further replans wait, in ID order.
    pub replan_budget: usize,
    /// Ticks an agent waits at a closed door/elevator before replanning around it.
    pub link_wait_ticks: u32,
    /// Ticks of wanting to move without moving before a forced replan.
    pub stall_ticks: u32,
    /// Ticks between detecting a carve/area obstacle change and publishing the
    /// rebaked mesh. The background job is joined at that boundary if late, so
    /// publication is deterministic.
    pub rebake_latency_ticks: u32,
}
impl Default for NavConfig {
    fn default() -> Self {
        Self {
            fixed_dt: 1.0 / 60.0,
            authority: Authority::Server,
            allow_mesh_swap: true,
            crowd: CrowdSolver::Orca,
            crowd_params: ara_crowd::Params::default(),
            replan_budget: 256,
            link_wait_ticks: 300,
            stall_ticks: 120,
            rebake_latency_ticks: 6,
        }
    }
}

/// Mesh handles and disposable solver scratch; gameplay truth stays in components.
#[derive(Resource)]
pub struct NavWorld {
    pub(crate) meshes: BTreeMap<u32, NavMesh>,
    pub(crate) pending: BTreeMap<u32, NavMesh>,
    pub(crate) config: NavConfig,
    pub(crate) agents: Vec<(u64, Entity)>,
    pub(crate) scratch: Vec<ara_nav_sys::Agent>,
    pub(crate) entities: Vec<Entity>,
    pub(crate) errors: Vec<(u64, NavError)>,
    pub(crate) gates: LinkGates,
    pub(crate) types: BTreeMap<u32, QueryFilter>,
    pub(crate) crowd: ara_crowd::Crowd,
    pub(crate) crowd_agents: Vec<ara_crowd::Agent>,
    pub(crate) obstacle_discs: Vec<ara_crowd::Agent>,
    pub(crate) id_query: Option<bevy_ecs::query::QueryState<&'static NavAgentState>>,
    pub(crate) ids: Vec<u64>,
    pub(crate) wants: Vec<crate::orca_step::Want>,
    pub(crate) requests: Vec<crate::PathRequest>,
    pub(crate) filters: Vec<QueryFilter>,
    pub(crate) waiting: BTreeMap<u32, u32>,
    pub(crate) agent_query: Option<AgentQuery>,
    pub(crate) rebakes: crate::obstacles::Rebakes,
    pub(crate) graphs: BTreeMap<u32, std::sync::Arc<crate::ClusterGraph>>,
    pub(crate) route_scratch: crate::RouteScratch,
    pub(crate) flows: BTreeMap<u32, std::sync::Arc<ara_crowd::FlowField>>,
}
impl NavWorld {
    /// Creates a world with a valid fixed tick; no workers are started here.
    pub fn new(config: NavConfig) -> Result<Self, NavError> {
        if !config.fixed_dt.is_finite() || config.fixed_dt <= 0.0 || config.fixed_dt > 0.25 {
            return Err(NavError::new(
                "invalidTick",
                "fixed dt must be in (0, 0.25]",
            ));
        }
        let crowd = ara_crowd::Crowd::new(config.crowd_params)
            .map_err(|e| NavError::new("invalidCrowd", e.0))?;
        Ok(Self {
            meshes: BTreeMap::new(),
            pending: BTreeMap::new(),
            config,
            agents: Vec::new(),
            scratch: Vec::new(),
            entities: Vec::new(),
            errors: Vec::new(),
            gates: LinkGates::default(),
            types: BTreeMap::new(),
            crowd,
            crowd_agents: Vec::new(),
            obstacle_discs: Vec::new(),
            id_query: None,
            ids: Vec::new(),
            wants: Vec::new(),
            requests: Vec::new(),
            filters: Vec::new(),
            waiting: BTreeMap::new(),
            agent_query: None,
            rebakes: Default::default(),
            graphs: BTreeMap::new(),
            route_scratch: Default::default(),
            flows: BTreeMap::new(),
        })
    }
    /// Stages an already-assembled background result; latest result wins per profile.
    pub fn stage(&mut self, mesh: NavMesh) {
        self.pending.insert(mesh.profile().id, mesh);
    }
    /// Publishes `mesh` now, replacing its profile's mesh and any staged one. For
    /// world load and checkpoint restore, outside a tick; during play, `stage` it so
    /// it publishes at the next tick boundary.
    pub fn publish(&mut self, mesh: NavMesh) {
        let profile = mesh.profile().id;
        self.pending.remove(&profile);
        self.meshes.insert(profile, mesh);
    }
    /// Controls only publication; use false during Play or while approval is pending.
    pub fn allow_mesh_swap(&mut self, allow: bool) {
        self.config.allow_mesh_swap = allow;
    }
    /// Changes authority without creating a second simulation state.
    pub fn set_authority(&mut self, authority: Authority) {
        self.config.authority = authority;
    }
    /// Current configuration.
    pub fn config(&self) -> &NavConfig {
        &self.config
    }
    /// Installs the always-resident cluster graph of a streamed profile. Agents whose
    /// goal lies outside the resident mesh then follow a long [`crate::Route`].
    pub fn set_cluster_graph(&mut self, profile: u32, graph: std::sync::Arc<crate::ClusterGraph>) {
        self.graphs.insert(profile, graph);
    }
    /// Publishes a shared flow field; agents with `NavAgentState::flow == Some(id)`
    /// steer by it (ORCA solver). Build fields off the tick thread.
    pub fn set_flow_field(&mut self, id: u32, field: std::sync::Arc<ara_crowd::FlowField>) {
        self.flows.insert(id, field);
    }
    /// Removes a flow field; its followers become unreachable until retargeted.
    pub fn remove_flow_field(&mut self, id: u32) {
        self.flows.remove(&id);
    }
    /// Current immutable mesh for a profile, if published.
    pub fn mesh(&self, profile: u32) -> Option<&NavMesh> {
        self.meshes.get(&profile)
    }
    /// Structured failures from the most recent tick, paired with stable agent IDs.
    pub fn errors(&self) -> &[(u64, NavError)] {
        &self.errors
    }
}

/// Publishes permitted completed bakes and advances server agents by one fixed tick.
/// Stable component IDs define order; ECS allocation/iteration order has no effect.
pub fn tick(world: &mut World) {
    if !world.contains_resource::<NavWorld>() {
        return;
    }
    world.resource_scope(
        |world, mut nav: bevy_ecs::change_detection::Mut<NavWorld>| {
            let nav = &mut *nav;
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
            crate::obstacles::update(world, nav);
            gather_gates(world, nav);
            nav.types.clear();
            for t in world.query::<&NavAgentType>().iter(world) {
                nav.types.insert(t.profile, t.filter());
            }
            match nav.config.crowd {
                CrowdSolver::Orca => crate::orca_step::step(world, nav),
                CrowdSolver::Detour => crate::detour_step::step(world, nav),
            }
            for mut gate in world.query::<&mut NavLinkState>().iter_mut(world) {
                let waiting = nav.waiting.get(&gate.id).copied().unwrap_or(0);
                if gate.waiting != waiting {
                    gate.waiting = waiting;
                }
            }
        },
    );
}

/// Collects link gates: closed links, and links planning must avoid.
fn gather_gates(world: &mut World, nav: &mut NavWorld) {
    let mut kinds: BTreeMap<u32, LinkKind> = BTreeMap::new();
    for mesh in nav.meshes.values() {
        for l in mesh.links() {
            kinds.insert(l.id, l.kind);
        }
    }
    nav.gates.closed.clear();
    nav.gates.blocked.clear();
    for gate in world.query::<&NavLinkState>().iter(world) {
        let waitable = kinds.get(&gate.id).is_some_and(|k| k.waitable());
        if !gate.open {
            nav.gates.closed.push(gate.id);
        }
        if gate.locked || (!gate.open && !waitable) {
            nav.gates.blocked.push(gate.id);
        }
    }
    nav.gates.closed.sort_unstable();
    nav.gates.closed.dedup();
    nav.gates.blocked.sort_unstable();
    nav.gates.blocked.dedup();
}

/// Registers the resource and the exclusive tick-boundary system with `ara-sim`'s
/// fixed schedule. Call once; order `NavigationTick` after transforms and before physics extraction.
pub fn register_sim(
    world: &mut World,
    schedule: &mut Schedule,
    config: NavConfig,
) -> Result<(), NavError> {
    if world.contains_resource::<NavWorld>() {
        return Err(NavError::new(
            "alreadyRegistered",
            "navigation is already registered",
        ));
    }
    world.insert_resource(NavWorld::new(config)?);
    schedule.add_systems(tick.in_set(NavigationTick));
    Ok(())
}

/// Goal accepted by the high-level automated-playtest movement driver.
#[derive(Debug, Clone, Copy)]
pub enum GotoTarget {
    /// A world-space point.
    Point([f32; 3]),
    /// Resolve this entity's current `NavPosition` when submitting the intent.
    Entity(Entity),
}
/// Submits a runtime movement intent. Documents are never edited; only an
/// authoritative world may accept it. Entity targets resolve once at submission.
pub fn goto(world: &mut World, agent: Entity, target: GotoTarget) -> Result<(), NavError> {
    let nav = world
        .get_resource::<NavWorld>()
        .ok_or_else(|| NavError::new("notRegistered", "navigation is not registered"))?;
    if nav.config.authority != Authority::Server {
        return Err(NavError::new("authority", "goto requires server authority"));
    }
    let point = match target {
        GotoTarget::Point(p) => p,
        GotoTarget::Entity(e) => {
            world
                .get::<NavPosition>(e)
                .ok_or_else(|| {
                    NavError::new("missingPosition", "goto target has no navigation position")
                })?
                .0
        }
    };
    if !finite(point) {
        return Err(NavError::new("invalidPoint", "goto target must be finite"));
    }
    if world.get::<NavAgent>(agent).is_none() || world.get::<NavPosition>(agent).is_none() {
        return Err(NavError::new(
            "missingAgent",
            "goto requires NavAgent, NavPosition and NavAgentState",
        ));
    }
    let mut state = world
        .get_mut::<NavAgentState>(agent)
        .ok_or_else(|| NavError::new("missingAgent", "agent has no navigation state"))?;
    state.set_target(Some(point));
    Ok(())
}
