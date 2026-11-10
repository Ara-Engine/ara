//! The backend-neutral shared-surface surface: device-taking, handle-based methods
//! with the same names and contracts as `metal::external`, so call sites compile
//! against `GpuDevice` on every backend. Each forwards to the native method it names.
use super::{ExportRing, ImportedSurface};
use crate::shared::{Frame, SharedSurface};
use crate::vulkan::VulkanDevice;
use crate::{Image, Result};

impl ExportRing {
    /// A first-generation ring of `size` RGBA8 slots on `gpu` ([`Self::new`]).
    /// # Safety
    /// `gpu` outlives the ring; its graphics queue is used from this thread only.
    pub unsafe fn create(gpu: &VulkanDevice, size: [u32; 2]) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        unsafe { Self::new(gpu.context(), size) }
    }
    /// The next generation at `size` ([`Self::resize`]).
    /// # Safety
    /// As [`Self::create`], with the same `gpu`.
    pub unsafe fn recreate(&mut self, gpu: &VulkanDevice, size: [u32; 2]) -> Result<()> {
        // SAFETY: forwarded caller contract.
        unsafe { self.resize(gpu.context(), size) }
    }
    /// Publish an engine image ([`Self::publish_scaled`]).
    /// # Safety
    /// `source` is a completed RGBA8 `CopySource` image of `source_size` on `gpu`, and
    /// this thread owns its graphics queue.
    pub unsafe fn publish_image(
        &mut self,
        gpu: &VulkanDevice,
        source: Image,
        source_size: [u32; 2],
        tick: u64,
    ) -> Result<Option<Frame>> {
        let source = gpu.native_image(source)?;
        // SAFETY: forwarded caller contract for the native image of `source`.
        unsafe { self.publish_scaled(source, source_size, tick) }
    }
}

impl ImportedSurface {
    /// Import every slot of `surface` on `gpu` ([`Self::new`]).
    /// # Safety
    /// `gpu` outlives the surface; its graphics queue is used from this thread only.
    pub unsafe fn create(gpu: &VulkanDevice, surface: &SharedSurface) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        unsafe { Self::new(gpu.context(), surface) }
    }
    /// Copy a published slot into the engine image `destination` (RGBA8 of the
    /// surface's size, in `CopySource`) and hand the slot back ([`Self::copy_out`]).
    /// # Safety
    /// As [`Self::copy_out`], with `gpu` the device this surface was imported on.
    pub unsafe fn copy_out_image(
        &mut self,
        gpu: &VulkanDevice,
        slot: u32,
        destination: Image,
    ) -> Result<()> {
        let destination = gpu.native_image(destination)?;
        // SAFETY: forwarded caller contract for the native image of `destination`.
        unsafe { self.copy_out(slot, destination) }
    }
    /// Read an owned slot as packed RGBA8 rows ([`Self::read`]).
    /// # Safety
    /// This thread owns the import's graphics queue.
    pub unsafe fn read_pixels(&mut self, _gpu: &VulkanDevice, slot: u32) -> Result<Vec<u8>> {
        // SAFETY: forwarded caller contract.
        unsafe { self.read(slot) }
    }
}
