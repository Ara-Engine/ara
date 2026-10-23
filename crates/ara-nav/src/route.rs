//! Long routes across the cluster graph, followed leg by leg on the resident mesh.
use serde::{Deserialize, Serialize};

/// A long route across clusters: world-space portal waypoints from the agent
/// toward a goal outside the resident mesh. Lives in `NavAgentState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    /// Portal crossings in travel order, ending at the goal.
    pub waypoints: Vec<[f32; 3]>,
    /// Index of the next waypoint to reach.
    pub next: usize,
    /// Estimated remaining path length in metres when planned.
    pub cost: f32,
}
