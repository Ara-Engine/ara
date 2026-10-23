//! Inspector declarations and snapshot-friendly authoritative runtime components.
use crate::NavPath;
use ara_schema::{AssetRef, Schema, SchemaDb};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Dimensions, speed and permitted areas of one navigation agent.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "navigation")]
#[serde(default, rename_all = "camelCase")]
pub struct NavAgent {
    /// Mesh profile ID; radius and height must fit that profile.
    pub profile: u32,
    /// Collision radius in metres.
    #[ara(default=0.3,unit=Meters,range(min=0.001,max=10))]
    pub radius: f32,
    /// Required clearance in metres.
    #[ara(default=1.8,unit=Meters,range(min=0.01,max=100))]
    pub height: f32,
    /// Maximum movement speed in metres per second.
    #[ara(default = 3, range(min = 0, max = 1000))]
    pub max_speed: f32,
    /// Maximum change in velocity per second.
    #[ara(default = 8, range(min = 0, max = 10000))]
    pub acceleration: f32,
    /// Areas this agent can use (one bit per area).
    #[ara(default = 65535)]
    pub area_mask: u16,
    /// Arrival tolerance in metres.
    #[ara(default=0.1,unit=Meters,range(min=0.001,max=10))]
    pub stopping_distance: f32,
}
impl Default for NavAgent {
    fn default() -> Self {
        Self {
            profile: 0,
            radius: 0.3,
            height: 1.8,
            max_speed: 3.0,
            acceleration: 8.0,
            area_mask: u16::MAX,
            stopping_distance: 0.1,
        }
    }
}

/// Authored raw-geometry navigation surface; its derived mesh is content-addressed.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "navigation")]
#[serde(default, rename_all = "camelCase")]
pub struct NavMeshSurface {
    /// Shared bake profiles to generate from this surface.
    pub profiles: Vec<u32>,
    /// Source collision geometry asset, resolved by the asset/physics integration.
    pub geometry: Option<AssetRef>,
    /// World-space lower bake bound.
    #[ara(unit=Meters)]
    pub min: [f32; 3],
    /// World-space upper bake bound.
    #[ara(unit=Meters)]
    pub max: [f32; 3],
    /// Horizontal voxel size in metres.
    #[ara(default=0.2,unit=Meters,range(min=0.01,max=10))]
    pub cell_size: f32,
    /// Interior tile width in voxels.
    #[ara(default = 64, range(min = 8, max = 256))]
    pub tile_size: u16,
}
impl Default for NavMeshSurface {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            geometry: None,
            min: [0.0; 3],
            max: [0.0; 3],
            cell_size: 0.2,
            tile_size: 64,
        }
    }
}

/// Authored off-mesh connection in world space (jump, ladder or scripted passage).
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "navigation")]
#[serde(default, rename_all = "camelCase")]
pub struct NavLink {
    /// Stable nonzero connection ID.
    pub id: u32,
    /// Start endpoint in world space.
    #[ara(unit=Meters)]
    pub start: [f32; 3],
    /// End endpoint in world space.
    #[ara(unit=Meters)]
    pub end: [f32; 3],
    /// Maximum horizontal attachment distance.
    #[ara(default=0.5,unit=Meters,range(min=0.001,max=100))]
    pub radius: f32,
    /// Traversal area index, from 0 through 15.
    #[ara(range(min = 0, max = 15))]
    pub area: u8,
    /// Whether traversal is allowed in both directions.
    #[ara(default = true)]
    pub bidirectional: bool,
    /// Whether the connection is available to the baker.
    #[ara(default = true)]
    pub enabled: bool,
    /// How agents traverse it: drives traversal motion, gating and animation hooks.
    pub kind: crate::LinkKind,
    /// Traversal time in seconds; 0 derives it from length and agent speed.
    #[ara(default=0,unit=Seconds,range(min=0,max=600))]
    pub duration: f32,
    /// Jump apex above the straight start-end line, in metres (jumps only).
    #[ara(default=0.5,unit=Meters,range(min=0,max=100))]
    pub arc_height: f32,
}
impl Default for NavLink {
    fn default() -> Self {
        Self {
            id: 0,
            start: [0.0; 3],
            end: [0.0; 3],
            radius: 0.5,
            area: 0,
            bidirectional: true,
            enabled: true,
            kind: crate::LinkKind::Walk,
            duration: 0.0,
            arc_height: 0.5,
        }
    }
}
impl From<&NavLink> for crate::OffMeshLink {
    fn from(l: &NavLink) -> Self {
        Self {
            id: l.id,
            start: l.start,
            end: l.end,
            radius: l.radius,
            area: l.area,
            bidirectional: l.bidirectional,
            enabled: l.enabled,
            kind: l.kind,
            duration: l.duration,
            arc_height: l.arc_height,
        }
    }
}

/// Area label and cost policy for the collision-geometry producer and query filters.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "navigation")]
#[serde(default, rename_all = "camelCase")]
pub struct NavArea {
    /// Semantic area ID, from 0 through 15.
    #[ara(range(min = 0, max = 15))]
    pub area: u8,
    /// Inspector name (for example Ground, Water or Hazard).
    pub name: String,
    /// Default query traversal cost; agents can override it in their state filter.
    #[ara(default = 1, range(min = 1, max = 1000000))]
    pub cost: f32,
}
impl Default for NavArea {
    fn default() -> Self {
        Self {
            area: 0,
            name: String::new(),
            cost: 1.0,
        }
    }
}

/// World-space navigation position, synchronized with the transform/physics track.
#[derive(Debug, Clone, Copy, PartialEq, Component, Serialize, Deserialize)]
pub struct NavPosition(pub [f32; 3]);

/// Result of an authoritative movement intent, suitable for playtest assertions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavStatus {
    /// No movement target.
    #[default]
    Idle,
    /// Following a complete path.
    Moving,
    /// Reached the requested goal within the stopping distance.
    Arrived,
    /// No complete permitted path, invalid state or no mesh; inspect diagnostics.
    Unreachable,
}

/// Explicit off-mesh traversal state; lives in ECS for snapshot/replay. Animation
/// systems read `kind` and `progress()` as the traversal hook (jump, climb, ride).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Traversal {
    /// Connection's starting position.
    pub start: [f32; 3],
    /// Connection's ending position.
    pub end: [f32; 3],
    /// Elapsed fixed-tick traversal time in seconds.
    pub elapsed: f32,
    /// Total traversal duration in seconds.
    pub duration: f32,
    /// Authored link ID, zero when unknown.
    #[serde(default)]
    pub link: u32,
    /// Traversal style.
    #[serde(default)]
    pub kind: crate::LinkKind,
    /// Jump apex above the straight line in metres.
    #[serde(default)]
    pub arc_height: f32,
}
impl Traversal {
    /// Normalised progress in 0..=1.
    pub fn progress(&self) -> f32 {
        if self.duration > 0.0 {
            (self.elapsed / self.duration).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
    /// Position at the current progress, including the jump arc.
    pub fn position(&self) -> [f32; 3] {
        let f = self.progress();
        let mut p: [f32; 3] =
            std::array::from_fn(|i| self.start[i] + (self.end[i] - self.start[i]) * f);
        if self.kind == crate::LinkKind::Jump {
            p[1] += 4.0 * self.arc_height * f * (1.0 - f);
        }
        p
    }
}

/// Complete authoritative navigation gameplay state; include it in snapshots.
#[derive(Debug, Clone, PartialEq, Component, Serialize, Deserialize)]
pub struct NavAgentState {
    /// Stable simulation ID; unique within a world and independent of Entity order.
    pub id: u64,
    /// Last authoritative velocity, in metres per second.
    pub velocity: [f32; 3],
    /// Requested goal, if any.
    pub target: Option<[f32; 3]>,
    /// Area costs/masks for this agent.
    pub filter: crate::QueryFilter,
    /// Current corridor's funnel waypoints (no hidden native path state).
    pub path: Option<NavPath>,
    /// Next waypoint index.
    pub corner: usize,
    /// Mesh content used to compute the path; changes force a replan.
    pub mesh: Option<crate::CacheKey>,
    /// Area filter used to plan the current corridor; changes force replanning.
    pub planned_filter: Option<crate::QueryFilter>,
    /// Explicit current off-mesh traversal, if any.
    pub traversal: Option<Traversal>,
    /// Last movement outcome.
    pub status: NavStatus,
    /// Path-following velocity before avoidance: the locomotion intent input.
    #[serde(default)]
    pub desired: [f32; 3],
    /// Consecutive ticks with a desired velocity but almost no movement.
    #[serde(default)]
    pub stalled: u32,
    /// Link the agent is waiting at (closed door, absent elevator), with ticks waited.
    #[serde(default)]
    pub waiting: Option<(u32, u32)>,
    /// Link excluded from the next plan after waiting too long at it.
    #[serde(default)]
    pub avoid_link: Option<u32>,
    /// Why the agent is not progressing, for diagnostics and `ara why`.
    #[serde(default)]
    pub issue: Option<NavIssue>,
    /// Long route across clusters when the goal is outside the resident mesh.
    #[serde(default)]
    pub route: Option<crate::Route>,
    /// Runtime speed cap below `NavAgent::max_speed` (AI gaits: walk, run).
    #[serde(default)]
    pub speed_limit: Option<f32>,
    /// Shared flow field (`NavWorld::set_flow_field`) steering this agent instead
    /// of its own corridor: massed groups with one destination skip planning.
    #[serde(default)]
    pub flow: Option<u32>,
}
impl NavAgentState {
    /// Creates idle deterministic state with a stable simulation ID.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            velocity: [0.0; 3],
            target: None,
            filter: crate::QueryFilter::default(),
            path: None,
            corner: 0,
            mesh: None,
            planned_filter: None,
            traversal: None,
            status: NavStatus::Idle,
            desired: [0.0; 3],
            stalled: 0,
            waiting: None,
            avoid_link: None,
            issue: None,
            route: None,
            speed_limit: None,
            flow: None,
        }
    }
    /// Follows shared flow field `field` toward `goal` (its arrival point).
    pub fn follow_flow(&mut self, field: u32, goal: [f32; 3]) {
        self.set_target(Some(goal));
        self.flow = Some(field);
    }
    /// Effective maximum speed: the authored speed capped by `speed_limit`.
    pub fn max_speed(&self, agent: &NavAgent) -> f32 {
        self.speed_limit
            .filter(|l| l.is_finite() && *l >= 0.0)
            .map_or(agent.max_speed, |l| l.min(agent.max_speed))
    }
    /// Sets a new goal, clearing the corridor so the next tick plans afresh.
    pub fn set_target(&mut self, target: Option<[f32; 3]>) {
        self.target = target;
        self.flow = None;
        self.path = None;
        self.route = None;
        self.corner = 0;
        self.traversal = None;
        self.waiting = None;
        self.avoid_link = None;
        self.stalled = 0;
        self.issue = None;
        self.status = if target.is_some() {
            NavStatus::Moving
        } else {
            NavStatus::Idle
        };
    }
}

/// Structured reason an agent is not progressing; kept in `NavAgentState`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum NavIssue {
    /// No mesh is published for the agent's profile.
    NoMesh,
    /// Agent dimensions or state are invalid for its mesh.
    InvalidAgent,
    /// The agent is not on (or near) a permitted polygon.
    StartOffMesh,
    /// The goal is not reachable; `reachable` is the nearest reachable point.
    NoPath {
        /// Closest point a permitted corridor reaches.
        reachable: [f32; 3],
    },
    /// Waiting at a closed door or absent elevator.
    WaitingForLink {
        /// Authored link ID.
        link: u32,
    },
    /// Gave up waiting at a link and replanned without it.
    LinkTimeout {
        /// Authored link ID.
        link: u32,
    },
    /// Desired motion but no progress (crowding or an obstacle); replanning.
    Stalled,
    /// Goal is outside the resident mesh; following a long route.
    OutsideResident,
}

/// Registers navigation authoring components with the kernel Schema DB.
pub fn register(db: &mut SchemaDb) {
    db.register::<NavAgent>()
        .register::<NavMeshSurface>()
        .register::<NavLink>()
        .register::<NavArea>()
        .register::<crate::NavObstacle>()
        .register::<crate::NavAgentType>();
}
