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

/// Explicit off-mesh interpolation state; lives in ECS for snapshot/replay.
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
        }
    }
}

/// Registers navigation authoring components with the kernel Schema DB.
pub fn register(db: &mut SchemaDb) {
    db.register::<NavAgent>()
        .register::<NavMeshSurface>()
        .register::<NavLink>()
        .register::<NavArea>();
}
