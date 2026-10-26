//! Native allocation shared by owner-thread reuse and unpublished worker jobs.
use super::{BufferAllocation, ImageAllocation};
use anyhow::{Context, Result};
use ash::vk;

mod buffer;
mod image;
mod upload;
mod validate;
pub(super) use buffer::buffer;
pub(crate) use image::image;
pub(crate) use upload::upload;
pub(crate) use validate::validate_image;

#[derive(Clone, Copy)]
pub(crate) struct ImageDesc {
    pub size: [u32; 2],
    pub format: vk::Format,
    pub mip_levels: u32,
    pub usage: vk::ImageUsageFlags,
    pub aspect: vk::ImageAspectFlags,
}

impl ImageDesc {
    pub fn new(
        size: [u32; 2],
        format: vk::Format,
        mip_levels: u32,
        usage: vk::ImageUsageFlags,
    ) -> Self {
        Self {
            size,
            format,
            mip_levels,
            usage,
            aspect: if format == vk::Format::D32_SFLOAT {
                vk::ImageAspectFlags::DEPTH
            } else {
                vk::ImageAspectFlags::COLOR
            },
        }
    }
}

/// Immutable native allocation access; the creating context owns device lifetime.
pub(crate) struct Allocator {
    pub device: ash::Device,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    memory: vk::PhysicalDeviceMemoryProperties,
    compressed_bc: bool,
    max_dimension: u32,
}

impl Allocator {
    pub fn buffer(
        &self,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: bool,
    ) -> crate::Result<BufferAllocation> {
        Ok(buffer(
            &self.device,
            &self.memory,
            &mut super::buffer_memory::BufferMemory::default(),
            size,
            usage,
            host,
            vk::MemoryPropertyFlags::empty(),
        )?)
    }

    pub fn image(&self, desc: ImageDesc) -> crate::Result<ImageAllocation> {
        validate_image(
            &self.instance,
            self.physical,
            self.compressed_bc,
            self.max_dimension,
            &desc,
        )?;
        Ok(image(&self.device, &self.memory, desc)?)
    }
}

impl super::Gpu {
    pub(crate) fn allocator(&self) -> Allocator {
        Allocator {
            device: self.device.clone(),
            instance: self.instance.clone(),
            physical: self.physical,
            memory: self.memory,
            compressed_bc: self.info.texture_compression_bc,
            max_dimension: self.info.max_image_dimension,
        }
    }

    pub(crate) fn validate_image(&self, desc: &ImageDesc) -> crate::Result<()> {
        validate_image(
            &self.instance,
            self.physical,
            self.info.texture_compression_bc,
            self.info.max_image_dimension,
            desc,
        )
    }

    pub(crate) fn track_buffer(&mut self, buffer: BufferAllocation) {
        self.buffers.push(buffer);
    }
    pub(crate) fn track_image(&mut self, image: ImageAllocation) {
        self.images.push(image);
    }
}

pub(super) fn memory_type(
    memory: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32> {
    (0..memory.memory_type_count)
        .find(|i| {
            bits & (1 << i) != 0
                && memory.memory_types[*i as usize]
                    .property_flags
                    .contains(flags)
        })
        .context("No matching memory type")
}
