//! Deterministic, parallel crowd avoidance for very large agent counts.
//!
//! [`Crowd::step`] runs ORCA (optimal reciprocal collision avoidance) for every
//! agent on the caller's Rayon pool. Each agent reads only the previous step's
//! positions and velocities and writes only its own result, so the output is
//! bit-identical for 1, 4 or 8 workers. Navmesh boundaries enter as [`Walls`]
//! (hard half-planes plus crossing clipping). [`FlowField`] steers many agents
//! toward one shared goal with a single Dijkstra.
//!
//! The crate is pure maths with no ECS, navmesh or native code: `ara-nav` gathers
//! agents from ECS in stable-ID order, calls `step`, and writes results back.
#![warn(missing_docs)]

mod flow;
mod grid;
mod orca;
mod solver;
mod walls;

pub use flow::{FlowField, MAX_CELLS, WalkGrid};
pub use solver::{Agent, Crowd, NO_WALLS, Params, Stats};
pub use walls::{Wall, Walls};

/// Rejected parameters, agents, walls or flow-field inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrowdError(pub String);
impl CrowdError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl std::fmt::Display for CrowdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CrowdError {}
