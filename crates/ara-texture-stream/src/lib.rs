//! Budgeted virtual texture pages, shared by mesh and landscape material passes.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
mod gpu;
mod native;
mod native_residency;
mod pool;
mod source;
mod types;
pub use gpu::Residency;
pub use native::{NativeMip, NativeSource};
pub use native_residency::{
    NativeBinding, NativeConfig, NativeResidency, NativeStats, NativeTextureId, NativeView,
};
pub use pool::{Pool, Upload};
pub use source::{Catalog, Source};
pub use types::{Binding, Stats};

/// Interior texels in each physical tile; coordinates always include a gutter.
pub const PAGE_SIZE: u32 = 128;
/// Physical tile extent including one wrapping texel on each side.
pub const TILE_SIZE: u32 = PAGE_SIZE + 2;
/// Uncompressed physical allocation per tile, including gutters.
pub const PAGE_BYTES: u64 = TILE_SIZE as u64 * TILE_SIZE as u64 * 4;
/// Sampling adapter for the existing Root32/bindless buffer ABI. Consumers keep
/// a copy in their offline source root and verify it equals this string.
pub fn shader_source() -> &'static str {
    include_str!("../shaders/sample.slang")
}
/// Native BC mip-tail sampling for Root32/bindless consumers. Copy into the
/// consumer's offline source root so cooking hashes the same sampling contract.
pub fn native_shader_source() -> &'static str {
    include_str!("../shaders/native_sample.slang")
}
/// Checks subgroup operations used to combine feedback without atomic contention.
/// Called at extraction preparation, before creating consumer pipelines.
pub fn validate_device(gpu: &ara_gpu::GpuDevice) -> anyhow::Result<()> {
    anyhow::ensure!(
        gpu.info().wave_ops,
        "ARA_TEXTURE_SUBGROUP: compute/fragment subgroup vote, ballot and arithmetic required"
    );
    Ok(())
}
