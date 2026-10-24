//! Device-owned compressed residency without a dependency on its asset loader.
use anyhow::Result;
use ara_gpu::{Access, Resource, vulkan::VulkanDevice};

/// Stable native texture table, whose rows identify the currently resident mip tails.
#[derive(Debug, Clone, Copy)]
pub struct TextureBinding {
    /// Storage-buffer heap index of the 32-byte texture rows.
    pub buffer_index: u32,
    /// Number of registered material texture IDs.
    pub textures: u32,
}

/// Payload and upload measurements for native compressed texture residency.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct TextureStats {
    /// Distinct cooked payload/view identities.
    pub textures: usize,
    /// Published compressed payload bytes.
    pub resident_bytes: u64,
    /// Payload requested before budget-driven coarsening.
    pub requested_bytes: u64,
    /// Images awaiting their budgeted mip tail.
    pub pending_textures: usize,
    /// Compressed bytes uploaded at the latest boundary.
    pub uploaded_bytes: u64,
    /// GPU staging duration at the latest boundary.
    pub upload_gpu_ms: f64,
}

/// Owned native textures for a renderer. Implementations upload compressed blocks
/// and publish only completed bindings; frame refinement does no asset IO or decode.
pub trait Textures: Send + std::any::Any {
    /// Address-independent cooked payload/view digest. Called at content publication.
    fn content_digest(&self) -> [u8; 32];
    /// Initialize every registered map before the first draw.
    fn initialize(&mut self, gpu: &mut VulkanDevice) -> Result<TextureStats>;
    /// Prepare a replacement against the completed resident set. Borrowed images
    /// must remain owned by `previous` until `publish_reuse`; destroying a rejected
    /// candidate must leave that previous set fully usable.
    fn initialize_reusing(
        &mut self,
        gpu: &mut VulkanDevice,
        _previous: &dyn Textures,
    ) -> Result<TextureStats> {
        self.initialize(gpu)
    }
    /// Transfer borrowed image ownership immediately before publishing this set.
    /// Errors must leave both sets unchanged; no rendering occurs between this
    /// transfer and replacing the old content. Providers without reuse do nothing.
    fn publish_reuse(&mut self, _previous: &mut dyn Textures) -> Result<()> {
        Ok(())
    }
    /// Refine mip tails at a completed frame boundary.
    fn refine(&mut self, gpu: &mut VulkanDevice) -> Result<TextureStats>;
    /// Last completed table, or none when no textures are registered.
    fn binding(&self) -> Option<TextureBinding>;
    /// Declare bindless reachability without allocating a resource list per frame.
    fn visit_resources(&self, visit: &mut dyn FnMut(Resource, Access));
    /// Retire the resident set after its final sampling submission completes.
    fn destroy(self: Box<Self>, gpu: &mut VulkanDevice) -> Result<()>;
}
