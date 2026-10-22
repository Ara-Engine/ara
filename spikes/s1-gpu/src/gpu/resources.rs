use super::Gpu;
use anyhow::{Context, Result, ensure};
use ash::vk;
use ash::vk::TaggedStructure;

/// Lightweight allocation reference; Gpu retains sole destruction ownership.
#[derive(Clone, Copy)]
pub struct Buffer {
    pub handle: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
}
/// Image, view and allocation retained together by Gpu.
#[derive(Clone, Copy)]
pub struct Image {
    pub handle: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
}

impl Gpu {
    /// Allocate an exclusive buffer in device-local or coherent host memory.
    pub fn buffer(&mut self, size: u64, usage: vk::BufferUsageFlags, host: bool) -> Result<Buffer> {
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
            let requirements = self.device.get_buffer_memory_requirements(handle);
            let flags = if host {
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
            } else {
                vk::MemoryPropertyFlags::DEVICE_LOCAL
            };
            let memory_type_index = self.memory_type(requirements.memory_type_bits, flags)?;
            let mut allocation_flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let mut allocation = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type_index);
            if usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS) {
                allocation = allocation.push(&mut allocation_flags);
            }
            let memory = self.device.allocate_memory(&allocation, None)?;
            self.device.bind_buffer_memory(handle, memory, 0)?;
            let buffer = Buffer {
                handle,
                memory,
                size,
            };
            self.buffers.push(buffer);
            Ok(buffer)
        }
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32> {
        (0..self.memory.memory_type_count)
            .find(|i| {
                bits & (1 << i) != 0
                    && self.memory.memory_types[*i as usize]
                        .property_flags
                        .contains(flags)
            })
            .context("No matching memory type")
    }

    /// Copy startup data into a completed coherent host allocation.
    pub fn upload(&self, buffer: Buffer, data: &[u8]) -> Result<()> {
        ensure!(data.len() as u64 <= buffer.size, "Upload out of bounds");
        // SAFETY: Caller uploads only before submission/after completion. The
        // host-coherent mapped allocation has enough capacity and is unmapped here.
        unsafe {
            let ptr = self.device.map_memory(
                buffer.memory,
                0,
                buffer.size,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.cast(), data.len());
            self.device.unmap_memory(buffer.memory);
        }
        Ok(())
    }

    /// Copy a completed coherent GPU readback into owned CPU bytes.
    pub fn read(&self, buffer: Buffer) -> Result<Vec<u8>> {
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
    pub fn address(&self, buffer: Buffer) -> u64 {
        // SAFETY: Used only for buffers allocated with SHADER_DEVICE_ADDRESS.
        unsafe {
            self.device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(buffer.handle),
            )
        }
    }

    /// Allocate a single-mip 2D image and its matching view.
    pub fn image(
        &mut self,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
    ) -> Result<Image> {
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
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = self.device.get_image_memory_requirements(handle);
            let memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(self.memory_type(
                        req.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )?),
                None,
            )?;
            self.device.bind_image_memory(handle, memory, 0)?;
            let view = self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(handle)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(range(aspect)),
                None,
            )?;
            let image = Image {
                handle,
                memory,
                view,
            };
            self.images.push(image);
            Ok(image)
        }
    }

    /// Create an AS with owned, address-enabled backing storage.
    pub fn structure(
        &mut self,
        size: u64,
        kind: vk::AccelerationStructureTypeKHR,
    ) -> Result<vk::AccelerationStructureKHR> {
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
            Ok(structure)
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
