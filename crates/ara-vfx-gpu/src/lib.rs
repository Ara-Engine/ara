#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Deterministic GPU particles over Ara's native GPU and render graph.
mod cook;
mod layout;
mod passes;
mod runtime;
mod readback;
mod stats;

pub use cook::cook_source;
pub use runtime::{GpuSystem, SceneDepth, View};
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
