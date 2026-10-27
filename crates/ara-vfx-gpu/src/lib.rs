#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Deterministic GPU particles over Ara's native GPU and render graph.
mod atlas;
mod cook;
mod layout;
mod passes;
mod pipeline_preparation;
mod readback;
mod resources;
mod runtime;
mod stats;

pub use atlas::{ATLAS_SIZE, cook_atlas};
pub use cook::cook_source;
pub use pipeline_preparation::{PipelineWarmup, PreparedPipelines};
pub use runtime::{GpuSystem, Lighting, Placement, ProbeLighting, SceneColor, SceneDepth, View};

/// HDR attachment format of previews, captures and the fidelity view: linear radiance.
pub const LINEAR_HDR: ara_gpu::vk::Format = ara_gpu::vk::Format::R32G32B32A32_SFLOAT;
/// HDR attachment format of the raster scene (pre-exposed, `View::hdr_exposure`).
pub const RASTER_HDR: ara_gpu::vk::Format = ara_gpu::vk::Format::R16G16B16A16_SFLOAT;

/// The cooked graphics recipes that draw into the system's HDR attachment.
pub(crate) const HDR_RECIPES: [&str; 2] = ["backdrop", "particles"];

/// A cooked recipe with its HDR colour attachment set to `hdr`: the cook is format-
/// agnostic, and the attachment format is chosen per system at PSO creation.
pub(crate) fn with_hdr_format(
    name: &str,
    mut recipe: ara_gpu::cooked::CookedPipeline,
    hdr: ara_gpu::vk::Format,
) -> ara_gpu::cooked::CookedPipeline {
    if HDR_RECIPES.contains(&name)
        && let Some(format) = recipe.state.color_formats.first_mut()
    {
        *format = hdr.as_raw();
    }
    recipe
}
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
