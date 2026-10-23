//! Cooked normalized pose/trajectory features, bounded SIMD search and inertial playback.
mod database;
mod features;
mod inertial;
pub(crate) mod player;
mod spring;
pub use database::{MatchConfig, MatchFrame, MotionDatabase, SearchResult};
pub use features::FeatureWriter;
pub use inertial::Inertializer;
pub use spring::{Spring, TrajectoryPoint, TrajectoryPredictor};

use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Graph-node search settings. The clip field references a cooked motion database.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchNode {
    /// Float parameter names in the database's raw feature order.
    pub query: Vec<String>,
    /// Maximum candidate rows examined per search; zero requests an exact full scan.
    pub budget: u32,
    /// Offset decay half-life in seconds.
    #[ara(default = 0.08)]
    pub half_life: f32,
    /// Keep playback when the selected frame is this many ticks from its continuation.
    #[ara(default = 1600)]
    pub continuation: u32,
}
impl Default for MatchNode {
    fn default() -> Self {
        Self {
            query: Vec::new(),
            budget: 0,
            half_life: 0.08,
            continuation: 1600,
        }
    }
}
