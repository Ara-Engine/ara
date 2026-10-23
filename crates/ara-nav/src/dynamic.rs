//! Dynamic-world vocabulary: typed links and their runtime gates, obstacles and
//! agent types with per-area costs.
use ara_schema::Schema;
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// How agents traverse an off-mesh link. Animation reads it from
/// `NavAgentState.traversal` (an intent), never through a call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Schema, Serialize, Deserialize)]
#[ara(module = "navigation")]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    /// Straight walk at agent speed (narrow passages, scripted moves).
    #[default]
    Walk,
    /// Ballistic arc with `arc_height`; plays a jump animation.
    Jump,
    /// Vertical climb at reduced speed; plays a ladder animation.
    Ladder,
    /// Passable only while open; agents wait at the start and request opening.
    Door,
    /// Passable only while the cabin is present; riders wait, then ride.
    Elevator,
    /// Instant relocation (portals, spawn tubes).
    Teleport,
}
impl LinkKind {
    /// Whether agents plan through a closed link of this kind and wait at it.
    pub fn waitable(self) -> bool {
        matches!(self, Self::Door | Self::Elevator)
    }
}

/// Runtime gate of a link (doors, elevators, collapsed bridges). Gameplay owns
/// `open`/`locked`; navigation writes `waiting` each tick so door controllers can
/// react to agents. Absent means open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Component, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NavLinkState {
    /// Stable link ID (matches `NavLink.id`).
    pub id: u32,
    /// Whether the link can be traversed now.
    pub open: bool,
    /// Never plan through it, even when the kind is waitable.
    pub locked: bool,
    /// Agents currently waiting at this link (written by the navigation tick).
    pub waiting: u32,
}
impl Default for NavLinkState {
    fn default() -> Self {
        Self {
            id: 0,
            open: true,
            locked: false,
            waiting: 0,
        }
    }
}

/// How the navigation reacts to a dynamic obstacle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Schema, Serialize, Deserialize)]
#[ara(module = "navigation")]
#[serde(rename_all = "camelCase")]
pub enum ObstacleMode {
    /// Cut the box out of the navmesh (background tile rebake). Paths go around.
    #[default]
    Carve,
    /// Relabel the box with `area`, whose per-type cost decides avoidance.
    Area,
    /// No rebake: local avoidance only (moving carts, temporary crowds).
    Avoid,
}

/// Dynamic obstacle: an axis-aligned box centred on the entity's `NavPosition`.
/// Carve/Area changes publish after `NavConfig::rebake_latency_ticks`; until then
/// (and always for `Avoid`) agents avoid it locally.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "navigation")]
#[serde(default, rename_all = "camelCase")]
pub struct NavObstacle {
    /// Stable nonzero ID; changes are canonically ordered by it.
    pub id: u32,
    /// Box half extents in metres.
    #[ara(default=[0.5,0.5,0.5],unit=Meters)]
    pub half_extents: [f32; 3],
    /// Carve, relabel or avoid only.
    pub mode: ObstacleMode,
    /// Area applied in `Area` mode, from 0 through 15.
    #[ara(range(min = 0, max = 15))]
    pub area: u8,
}
impl Default for NavObstacle {
    fn default() -> Self {
        Self {
            id: 0,
            half_extents: [0.5; 3],
            mode: ObstacleMode::Carve,
            area: 0,
        }
    }
}

/// A named agent type: its own baked mesh (dimensions) and traversal costs.
/// Agents select it through `NavAgent.profile == profile`.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "navigation")]
#[serde(default, rename_all = "camelCase")]
pub struct NavAgentType {
    /// Mesh profile ID shared by the type's agents and its bake.
    pub profile: u32,
    /// Inspector name (Humanoid, Brute, Drone).
    pub name: String,
    /// Bake radius in metres.
    #[ara(default=0.3,unit=Meters,range(min=0.01,max=10))]
    pub radius: f32,
    /// Bake clearance height in metres.
    #[ara(default=1.8,unit=Meters,range(min=0.1,max=100))]
    pub height: f32,
    /// Maximum step height in metres.
    #[ara(default=0.4,unit=Meters,range(min=0,max=10))]
    pub climb: f32,
    /// Steepest walkable slope in degrees.
    #[ara(default=45,unit=Degrees,range(min=0,max=89))]
    pub slope: f32,
    /// Cost multiplier per area ID (index 0..15); missing entries are 1.
    pub area_costs: Vec<f32>,
    /// Areas this type may enter (one bit per area).
    #[ara(default = 65535)]
    pub area_mask: u16,
}
impl Default for NavAgentType {
    fn default() -> Self {
        Self {
            profile: 0,
            name: String::new(),
            radius: 0.3,
            height: 1.8,
            climb: 0.4,
            slope: 45.0,
            area_costs: Vec::new(),
            area_mask: u16::MAX,
        }
    }
}
impl NavAgentType {
    /// Bake dimensions for this type's mesh.
    pub fn profile(&self) -> crate::AgentProfile {
        crate::AgentProfile {
            id: self.profile,
            radius: self.radius,
            height: self.height,
            climb: self.climb,
            slope: self.slope,
        }
    }
    /// The type's filter: costs and mask. Agents' own filters multiply onto it.
    pub fn filter(&self) -> crate::QueryFilter {
        let mut filter = crate::QueryFilter::default();
        for (slot, cost) in filter.costs.iter_mut().zip(&self.area_costs) {
            *slot = *cost;
        }
        filter.include = self.area_mask;
        filter
    }
}
