//! Brain authoring/runtime components and the intents brains write. Animation,
//! navigation and gameplay read intents; nothing calls into them directly.
use crate::program::Value;
use ara_schema::{AssetRef, Schema, SchemaDb};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Runs a behaviour on this entity (authoring).
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "ai")]
#[serde(default, rename_all = "camelCase")]
pub struct Brain {
    /// Behaviour graph asset (`@ai/guard.behavior#gr_...`).
    pub behavior: Option<AssetRef>,
    /// Blackboard overrides as `name = expression` (no IDs), e.g.
    /// `route = [vec3(0, 0, 4), vec3(8, 0, 4)]`.
    pub overrides: Vec<String>,
    /// Whether the brain thinks.
    #[ara(default = true)]
    pub enabled: bool,
}
impl Default for Brain {
    fn default() -> Self {
        Self {
            behavior: None,
            overrides: Vec::new(),
            enabled: true,
        }
    }
}
impl Brain {
    /// Program key: the asset ID part of the reference (`gr_...`), or the whole text.
    pub fn program_key(&self) -> Option<&str> {
        let text = self.behavior.as_ref()?.0.as_str();
        Some(text.rsplit_once('#').map_or(text, |(_, id)| id))
    }
}

/// Status of one active task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Still working.
    #[default]
    Running,
    /// Finished successfully (finishing tasks only).
    Succeeded,
    /// Gave up (unreachable, empty route, nothing found).
    Failed,
}

/// Per-task runtime state; reset when its state is (re-)entered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    /// Current status.
    pub status: TaskStatus,
    /// Ticks since the task started.
    pub ticks: u32,
    /// Route index (patrol) or fire count (act).
    pub index: u32,
    /// Chosen point (wander) or last commanded target.
    pub point: Option<[f32; 3]>,
    /// Where the task started (wander centre).
    pub origin: Option<[f32; 3]>,
}

/// The latest state change, for debugging and `ara why`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// Previous leaf state index (None on start).
    pub from: Option<u16>,
    /// New leaf state index.
    pub to: u16,
    /// Why it happened (`done`, `failed`, `when`, `select`, `start`).
    pub cause: String,
    /// `BrainState::age` when it happened.
    pub at: u64,
}

/// Complete runtime state of one brain: active path, task progress, blackboard
/// and RNG. Snapshot it with the entity; programs are shared, immutable data.
#[derive(Debug, Clone, PartialEq, Component, Serialize, Deserialize)]
pub struct BrainState {
    /// Stable simulation ID (same as navigation/perception).
    pub id: u64,
    /// Program key in `AiWorld`.
    pub program: String,
    /// Semantic hash the state was built for; a different program restarts it.
    pub hash: String,
    /// Active states, root to leaf (program state indices).
    pub active: Vec<u16>,
    /// Ticks spent in each active state.
    pub ticks: Vec<u32>,
    /// Ticks since each active state's selection was last re-evaluated.
    pub reselect: Vec<u32>,
    /// Task states of all active states, root first, in task order.
    pub tasks: Vec<TaskState>,
    /// Blackboard values by slot.
    pub vars: Vec<Value>,
    /// SplitMix64 state for `random()` and random selection.
    pub rng: u64,
    /// Ticks since the brain started.
    pub age: u64,
    /// Latest state change.
    pub last: Option<Change>,
    /// Latest utility scores of the deepest utility selector: (child, score).
    pub scores: Vec<(u16, f32)>,
}
impl BrainState {
    /// A brain that starts `program` on its first tick.
    pub fn new(id: u64, program: impl Into<String>) -> Self {
        Self {
            id,
            program: program.into(),
            hash: String::new(),
            active: Vec::new(),
            ticks: Vec::new(),
            reselect: Vec::new(),
            tasks: Vec::new(),
            vars: Vec::new(),
            rng: id ^ 0x5851_f42d_4c95_7f2d,
            age: 0,
            last: None,
            scores: Vec::new(),
        }
    }
}

/// Where the brain wants to go (bridged to navigation each tick).
#[derive(Debug, Clone, Copy, Default, PartialEq, Component, Serialize, Deserialize)]
pub struct MoveIntent {
    /// Destination, or None to stop.
    pub target: Option<[f32; 3]>,
    /// Speed cap in m/s (walk vs run).
    pub speed: Option<f32>,
}

/// Where the brain wants to look (perception forward and animation aim).
#[derive(Debug, Clone, Copy, Default, PartialEq, Component, Serialize, Deserialize)]
pub struct LookIntent {
    /// Look-at point, or None to face the movement direction.
    pub target: Option<[f32; 3]>,
}

/// Gameplay actions requested by `act` tasks; `sequence` increments per request
/// so consumers detect repeats without events.
#[derive(Debug, Clone, Default, PartialEq, Component, Serialize, Deserialize)]
pub struct ActionIntent {
    /// Latest action name (`fire`, `reload`).
    pub name: String,
    /// Optional target entity ID.
    pub target: Option<u64>,
    /// Monotonic request counter.
    pub sequence: u32,
    /// Latest `emit` event name and its counter.
    pub event: Option<(String, u32)>,
}

/// Locomotion goal for animation (motion matching reads this; no direct calls):
/// desired velocity from path following, facing and the active traversal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Component, Serialize, Deserialize)]
pub struct LocomotionIntent {
    /// Desired (pre-avoidance) velocity in m/s.
    pub desired: [f32; 3],
    /// Actual crowd velocity in m/s.
    pub velocity: [f32; 3],
    /// Unit facing direction on the XZ plane.
    pub facing: [f32; 3],
    /// Active off-mesh traversal (jump, ladder, door) and its progress.
    pub traversal: Option<(ara_nav::LinkKind, f32)>,
}

/// Registers reflected AI components.
pub fn register(db: &mut SchemaDb) {
    db.register::<Brain>();
    ara_perception::register(db);
}
