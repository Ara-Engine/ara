//! Stable-order fixed stepping with authoritative state exclusively in components.
use crate::{NavAgent, NavAgentState, NavError, NavMesh, NavPosition, NavStatus, finite, tick};
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
/// Fixed navigation tick and non-disruption publication policy.
#[derive(Debug, Clone, Copy)]
pub struct NavConfig {
    /// Fixed seconds per tick; set once, never derive from the wall clock.
    pub fixed_dt: f32,
    /// Whether this world runs authoritative AI.
    pub authority: Authority,
    /// Whether ready agent bakes may swap at this boundary (Play/approval gate).
    pub allow_mesh_swap: bool,
}
impl Default for NavConfig {
    fn default() -> Self {
        Self {
            fixed_dt: 1.0 / 60.0,
            authority: Authority::Server,
            allow_mesh_swap: true,
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
        Ok(Self {
            meshes: BTreeMap::new(),
            pending: BTreeMap::new(),
            config,
            agents: Vec::new(),
            scratch: Vec::new(),
            entities: Vec::new(),
            errors: Vec::new(),
        })
    }
    /// Stages an already-assembled background result; latest result wins per profile.
    pub fn stage(&mut self, mesh: NavMesh) {
        self.pending.insert(mesh.profile().id, mesh);
    }
    /// Controls only publication; use false during Play or while approval is pending.
    pub fn allow_mesh_swap(&mut self, allow: bool) {
        self.config.allow_mesh_swap = allow;
    }
    /// Changes authority without creating a second simulation state.
    pub fn set_authority(&mut self, authority: Authority) {
        self.config.authority = authority;
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
    state.target = Some(point);
    state.path = None;
    state.corner = 0;
    state.traversal = None;
    state.status = NavStatus::Moving;
    Ok(())
}
