use super::Gpu;
use super::buffer_memory::MemoryKey;
use anyhow::{Context, Result, ensure};
use ash::vk;

/// Lightweight allocation reference; Gpu retains sole destruction ownership.
#[derive(Clone, Copy)]
pub(crate) struct BufferAllocation {
    pub handle: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
    pub host: bool,
    pub usage: vk::BufferUsageFlags,
    pub(super) memory_key: Option<MemoryKey>,
}
/// ImageAllocation, view and allocation retained together by Gpu.
#[derive(Clone, Copy)]
pub(crate) struct ImageAllocation {
    pub handle: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub format: vk::Format,
    pub aspect: vk::ImageAspectFlags,
    pub usage: vk::ImageUsageFlags,
}

impl Gpu {
    /// Allocate an exclusive buffer in device-local or coherent host memory.
    pub(crate) fn buffer(
        &mut self,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: bool,
    ) -> Result<BufferAllocation> {
        self.buffer_preferred(size, usage, host, vk::MemoryPropertyFlags::empty())
    }

    pub(crate) fn readback_buffer(&mut self, size: u64) -> Result<BufferAllocation> {
        self.buffer_preferred(
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
            vk::MemoryPropertyFlags::HOST_CACHED,
        )
    }

    fn buffer_preferred(
        &mut self,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: bool,
        preferred: vk::MemoryPropertyFlags,
    ) -> Result<BufferAllocation> {
        let buffer = super::allocation::buffer(
            &self.shared.device,
            &self.shared.memory,
            &mut self.buffer_memory,
            size,
            usage,
            host,
            preferred,
        )?;
        self.buffers.push(buffer);
        Ok(buffer)
    }

    pub(crate) fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32> {
        (0..self.memory.memory_type_count)
            .find(|i| {
                bits & (1 << i) != 0
                    && self.memory.memory_types[*i as usize]
                        .property_flags
                        .contains(flags)
            })
            .context("No matching memory type")
    }

    /// Copy a byte range into a completed coherent host allocation.
    pub(crate) fn upload_range(
        &self,
        buffer: BufferAllocation,
        offset: u64,
        data: &[u8],
    ) -> Result<()> {
        super::allocation::upload(&self.device, buffer, offset, data)
    }

    /// Copy a completed coherent GPU readback into owned CPU bytes.
    pub(crate) fn read(&self, buffer: BufferAllocation) -> Result<Vec<u8>> {
        ensure!(buffer.host, "Buffer is not host visible");
        // SAFETY: Used only after the GPU timeline completion and a HOST_READ barrier.
        unsafe {
            let ptr = self.device.map_memory(
                buffer.memory,
                0,
                buffer.size,
                vk::MemoryMapFlags::empty(),
            )?;
            let bytes = std::slice::from_raw_parts(ptr.cast(), buffer.size as usize).to_vec();
            self.device.unmap_memory(buffer.memory);
            Ok(bytes)
        }
    }

    /// Query the address of an address-enabled buffer for AS construction.
    pub(crate) fn address(&self, buffer: BufferAllocation) -> u64 {
        // SAFETY: Used only for buffers allocated with SHADER_DEVICE_ADDRESS.
        unsafe {
            self.device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(buffer.handle),
            )
        }
    }

    /// Allocate a 2D image and a view spanning its complete resident mip chain.
    pub(crate) fn image(
        &mut self,
        width: u32,
        height: u32,
        format: vk::Format,
        mip_levels: u32,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
    ) -> Result<ImageAllocation> {
        let image = super::allocation::image(
            &self.device,
            &self.memory,
            super::allocation::ImageDesc {
                size: [width, height],
                format,
                mip_levels,
                usage,
                aspect,
            },
        )?;
        self.images.push(image);
        Ok(image)
    }

    /// Create an AS with owned, address-enabled backing storage.
    pub(crate) fn structure(
        &mut self,
        size: u64,
        kind: vk::AccelerationStructureTypeKHR,
    ) -> Result<(vk::AccelerationStructureKHR, BufferAllocation)> {
        ensure!(
            self.info.ray_tracing,
            "Device has no acceleration structure support"
        );
        let buffer = self.buffer(
            size,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        // SAFETY: Backing buffer stays alive until all structures are destroyed.
        unsafe {
            let structure = self.acceleration.create_acceleration_structure(
                &vk::AccelerationStructureCreateInfoKHR::default()
                    .buffer(buffer.handle)
                    .size(size)
                    .ty(kind),
                None,
            )?;
            self.structures.push(structure);
            Ok((structure, buffer))
        }
    }
}

pub fn range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(aspect)
        .level_count(1)
        .layer_count(1)
}

/// Construct one same-family image transition for synchronization2.
pub fn image_barrier(
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
    source: (vk::PipelineStageFlags2, vk::AccessFlags2),
    dest: (vk::PipelineStageFlags2, vk::AccessFlags2),
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .image(image)
        .subresource_range(range(aspect))
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .src_stage_mask(source.0)
        .src_access_mask(source.1)
        .dst_stage_mask(dest.0)
        .dst_access_mask(dest.1)
}

impl Gpu {
    /// Destroy a timeline-retired buffer, removing the teardown record.
    pub(crate) fn destroy_buffer(&mut self, buffer: BufferAllocation) {
        if let Some(memory) = self.release_buffer(buffer) {
            // SAFETY: release_buffer destroyed the completed buffer and did not
            // transfer its allocation to the bounded reuse pool.
            unsafe { self.device.free_memory(memory, None) };
        }
    }

    /// Destroy a completed buffer and retain reusable memory. The caller owns
    /// any returned uncached block and may free it on the disposal worker.
    pub(crate) fn release_buffer(&mut self, buffer: BufferAllocation) -> Option<vk::DeviceMemory> {
        self.buffers.retain(|b| b.handle != buffer.handle);
        // SAFETY: Caller proved every referencing queue completed before retirement.
        unsafe {
            self.device.destroy_buffer(buffer.handle, None);
            // Only memory is reused: a new exclusive VkBuffer starts with fresh
            // queue ownership and cannot inherit the retired buffer's access state.
        }
        (!buffer
            .memory_key
            .is_some_and(|key| self.buffer_memory.retain(key, buffer.memory)))
        .then_some(buffer.memory)
    }
    /// Transfers an already-retired allocation to the device's disposal worker.
    /// Removing the teardown record preserves sole destruction ownership.
    pub(crate) fn detach_image(&mut self, image: ImageAllocation) {
        self.images.retain(|i| i.handle != image.handle);
    }
}

impl Gpu {
    pub(crate) fn destroy_structure(
        &mut self,
        handle: vk::AccelerationStructureKHR,
        backing: BufferAllocation,
    ) {
        self.structures.retain(|h| *h != handle);
        // SAFETY: All referencing queue timelines completed, including TLAS dependency uses.
        unsafe {
            self.acceleration
                .destroy_acceleration_structure(handle, None);
        }
        self.destroy_buffer(backing);
    }
}
