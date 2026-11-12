//! Native BC material residency: budgeted compressed mip tails for mesh and terrain material passes.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
mod native;
mod native_residency;
pub use native::{NativeMip, NativeSource};
pub use native_residency::{
    NativeBinding, NativeConfig, NativeResidency, NativeStats, NativeTextureId, NativeView,
};

/// Native BC mip-tail sampling for Root32/bindless consumers. Copy into the
/// consumer's offline source root so cooking hashes the same sampling contract.
pub fn native_shader_source() -> &'static str {
    include_str!("../shaders/native_sample.slang")
}
