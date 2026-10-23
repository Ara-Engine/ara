//! Budgeted virtual texture pages, shared by mesh and landscape material passes.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
mod gpu;
mod pool;
mod source;
mod types;
pub use gpu::Residency;
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
/// Checks subgroup operations used to combine feedback without atomic contention.
/// Called at extraction preparation, before creating consumer pipelines.
pub fn validate_device(gpu: &ara_gpu::vulkan::VulkanDevice) -> anyhow::Result<()> {
    use ara_gpu::vk;
    use ara_gpu::vk::TaggedStructure;
    let mut properties = vk::PhysicalDeviceSubgroupProperties::default();
    // SAFETY: Read-only physical-device query; chained output lives through call.
    unsafe {
        gpu.context().instance.get_physical_device_properties2(
            gpu.context().physical,
            &mut vk::PhysicalDeviceProperties2::default().push(&mut properties),
        );
    }
    anyhow::ensure!(
        properties
            .supported_stages
            .contains(vk::ShaderStageFlags::COMPUTE | vk::ShaderStageFlags::FRAGMENT)
            && properties.supported_operations.contains(
                vk::SubgroupFeatureFlags::BASIC
                    | vk::SubgroupFeatureFlags::VOTE
                    | vk::SubgroupFeatureFlags::BALLOT
                    | vk::SubgroupFeatureFlags::ARITHMETIC
            ),
        "ARA_TEXTURE_SUBGROUP: compute/fragment subgroup vote, ballot and arithmetic required"
    );
    Ok(())
}
