//! Tiled Recast baking and server-authoritative navigation for Ara.
//!
//! Bake in [`BakeJob`]'s worker process, publish at a tick boundary, then query
//! immutable [`NavMesh`] snapshots in parallel. All crowd gameplay state lives
//! in ECS components. Positions use Y-up metres.
#![warn(missing_docs)]

mod artifact;
mod bake;
mod components;
mod debug;
mod detour_step;
mod diagnostic;
mod dynamic;
mod follow;
mod hier;
mod input;
mod job;
mod native;
#[cfg(test)]
mod native_tests;
mod obstacles;
mod orca_step;
mod query;
mod route;
mod stream;
mod world;

pub use ara_crowd;
pub use artifact::{BakeArtifact, BakeStats, CacheKey, TileData};
pub use bake::Baker;
pub use components::{
    NavAgent, NavAgentState, NavArea, NavIssue, NavLink, NavMeshSurface, NavPosition, NavStatus,
    Traversal, register,
};
pub use debug::DebugPrimitive;
pub use diagnostic::{Reachability, ReachabilityReason, why_unreachable};
pub use dynamic::{LinkKind, NavAgentType, NavLinkState, NavObstacle, ObstacleMode};
pub use hier::{
    CellCoord, CellGrid, ClusterGraph, ClusterSummary, LANDMARKS, MIN_PORTAL, Portal, RouteScratch,
    RouteStats, Side, UNREACHABLE,
};
pub use input::{
    AgentProfile, BakeInput, BakeSettings, Bounds, Geometry, Obstacle, OffMeshLink, TileCoord,
};
pub use job::{BakeJob, JobPoll, WorkerRequest, worker_main};
pub use obstacles::RebakeCheckpoint;
pub use query::{
    NavMesh, NavPath, NavQuery, NearestPoint, PathCorner, PathRequest, QueryFilter, RaycastHit,
};
pub use route::Route;
pub use stream::{CellBake, CellSource, NavStreamer, bake_cell, merge_cells};
pub use world::{
    Authority, CrowdSolver, GotoTarget, NavConfig, NavWorld, NavigationTick, goto, register_sim,
    tick,
};

/// A rejected bake, native allocation, query, job or integration operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NavError {
    /// Stable machine-readable error category (for example `invalidGeometry`).
    pub kind: String,
    /// Context for the editor, log or agent.
    pub message: String,
}

impl NavError {
    pub(crate) fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for NavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}
impl std::error::Error for NavError {}
impl From<std::io::Error> for NavError {
    fn from(error: std::io::Error) -> Self {
        Self::new("io", error.to_string())
    }
}

pub(crate) fn finite(point: [f32; 3]) -> bool {
    point.into_iter().all(f32::is_finite)
}
pub(crate) fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f32>()
        .sqrt()
}
