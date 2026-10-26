use super::Gpu;
use super::buffer_memory::MemoryKey;
use anyhow::{Context, Result, ensure};
use ash::vk;
use ash::vk::TaggedStructure;

/// Lightweight allocation reference; Gpu retains sole destruction ownership.
#[derive(Clone, Copy)]
pub(crate) struct BufferAllocation {
    pub handle: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
    pub host: bool,
    pub usage: vk::BufferUsageFlags,
    memory_key: Option<MemoryKey>,
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
        // SAFETY: Allocations satisfy queried requirements; resources are retained
        // until device idle in Drop. Exclusive ownership is managed in render.rs.
        unsafe {
            let handle = self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let mut memory = vk::DeviceMemory::null();
            let result = (|| -> Result<_> {
                let mut dedicated = vk::MemoryDedicatedRequirements::default();
                let mut requirements = vk::MemoryRequirements2::default().push(&mut dedicated);
                self.device.get_buffer_memory_requirements2(
                    &vk::BufferMemoryRequirementsInfo2::default().buffer(handle),
                    &mut requirements,
                );
                let requirements = requirements.memory_requirements;
                let flags = if host {
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
                } else {
                    vk::MemoryPropertyFlags::DEVICE_LOCAL
                };
                let memory_type_index = self
                    .memory_type(requirements.memory_type_bits, flags | preferred)
                    .or_else(|_| self.memory_type(requirements.memory_type_bits, flags))?;
                let key = MemoryKey {
                    size: requirements.size,
                    memory_type: memory_type_index,
                    device_address: usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS),
                };
                let memory_key =
                    (dedicated.requires_dedicated_allocation == vk::FALSE).then_some(key);
                let mut address_flags = vk::MemoryAllocateFlagsInfo::default()
                    .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
                let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().buffer(handle);
                let mut allocation = vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type_index);
                if key.device_address {
                    allocation = allocation.push(&mut address_flags);
                }
                if memory_key.is_none() {
                    allocation = allocation.push(&mut dedicated_info);
                }
                memory = match memory_key.and_then(|key| self.buffer_memory.take(key)) {
                    Some(memory) => memory,
                    None => self.device.allocate_memory(&allocation, None)?,
                };
                self.device.bind_buffer_memory(handle, memory, 0)?;
                Ok(BufferAllocation {
                    handle,
                    memory,
                    size,
                    host,
                    usage,
                    memory_key,
                })
            })();
            let buffer = match result {
                Ok(buffer) => buffer,
                Err(error) => {
                    self.device.destroy_buffer(handle, None);
                    if memory != vk::DeviceMemory::null() {
                        self.device.free_memory(memory, None);
                    }
                    return Err(error);
                }
            };
            self.buffers.push(buffer);
            Ok(buffer)
        }
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
        ensure!(buffer.host, "Buffer is not host visible");
        ensure!(
            offset
                .checked_add(data.len() as u64)
                .is_some_and(|end| end <= buffer.size),
            "Upload out of bounds"
        );
        // SAFETY: Caller uploads only before submission/after completion. The
        // host-coherent mapped allocation has enough capacity and is unmapped here.
        unsafe {
            let ptr = self.device.map_memory(
                buffer.memory,
                0,
                buffer.size,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                ptr.cast::<u8>().add(offset as usize),
                data.len(),
            );
            self.device.unmap_memory(buffer.memory);
        }
        Ok(())
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
        // SAFETY: Memory/view match queried image requirements. Gpu owns all three.
        unsafe {
            let handle = self.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(mip_levels)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let mut memory = vk::DeviceMemory::null();
            let mut view = vk::ImageView::null();
            let result = (|| -> Result<_> {
                let req = self.device.get_image_memory_requirements(handle);
                memory = self.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(self.memory_type(
                            req.memory_type_bits,
                            vk::MemoryPropertyFlags::DEVICE_LOCAL,
                        )?),
                    None,
                )?;
                self.device.bind_image_memory(handle, memory, 0)?;
                view = self.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(handle)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(range(aspect).level_count(mip_levels)),
                    None,
                )?;
                Ok(ImageAllocation {
                    handle,
                    memory,
                    view,
                    width,
                    height,
                    mip_levels,
                    format,
                    aspect,
                    usage,
                })
            })();
            let image = match result {
                Ok(image) => image,
                Err(error) => {
                    if view != vk::ImageView::null() {
                        self.device.destroy_image_view(view, None);
                    }
                    self.device.destroy_image(handle, None);
                    if memory != vk::DeviceMemory::null() {
                        self.device.free_memory(memory, None);
                    }
                    return Err(error);
                }
            };
            self.images.push(image);
            Ok(image)
        }
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
