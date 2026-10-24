#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Deterministic GPU particles over Ara's native GPU and render graph.
mod atlas;
mod cook;
mod layout;
mod passes;
mod readback;
mod resources;
mod runtime;
mod stats;

pub use atlas::{ATLAS_SIZE, cook_atlas};
pub use cook::cook_source;
pub use runtime::{GpuSystem, Lighting, Placement, ProbeLighting, SceneColor, SceneDepth, View};
pub use stats::{EmitterStats, Stats};

/// All runtime shader entries, cooked together under one immutable content key.
pub const COMPUTE_ENTRIES: &[&str] = &[
    "prepareMain",
    "spawnMain",
    "updateMain",
    "scanMain",
    "prefixMain",
    "scatterMain",
    "routeMain",
    "sortPrepareMain",
    "sortMain",
    "argsMain",
];
