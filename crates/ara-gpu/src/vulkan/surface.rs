use super::*;
use crate::descriptors::*;

impl BindlessHeap for VulkanDevice {
    fn insert_acceleration_structure(
        &mut self,
        structure: AccelerationStructure,
    ) -> Result<HeapIndex> {
        self.bind_acceleration_structure(structure)
    }
    fn insert_image(&mut self, image: Image) -> Result<HeapIndex> {
        self.sampled_image(image, false)
    }
    fn insert_buffer(&mut self, buffer: Buffer) -> Result<HeapIndex> {
        self.storage_buffer(buffer)
    }
    fn insert_storage_image(&mut self, image: Image) -> Result<HeapIndex> {
        self.storage_image(image)
    }
}
pub(super) fn format(format: Format) -> vk::Format {
    match format {
        Format::Rgba8 => vk::Format::R8G8B8A8_UNORM,
        Format::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        Format::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        Format::R32Float => vk::Format::R32_SFLOAT,
        Format::Depth32Float => vk::Format::D32_SFLOAT,
    }
}
impl VulkanDevice {
    pub(super) fn portable_buffer(&mut self, desc: &BufferDesc) -> Result<Buffer> {
        let mut usage = vk::BufferUsageFlags::empty();
        for (enabled, flag) in [
            (desc.vertex, vk::BufferUsageFlags::VERTEX_BUFFER),
            (
                desc.device_address,
                vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            ),
            (
                desc.acceleration_input,
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            ),
            (desc.index, vk::BufferUsageFlags::INDEX_BUFFER),
            (desc.storage, vk::BufferUsageFlags::STORAGE_BUFFER),
            (desc.copy_source, vk::BufferUsageFlags::TRANSFER_SRC),
            (desc.copy_destination, vk::BufferUsageFlags::TRANSFER_DST),
        ] {
            if enabled {
                usage |= flag;
            }
        }
        self.create_buffer(desc.size, usage, desc.host_visible)
    }
    pub(super) fn portable_image(&mut self, desc: &ImageDesc) -> Result<Image> {
        let mut usage = vk::ImageUsageFlags::empty();
        for (enabled, flag) in [
            (desc.sampled, vk::ImageUsageFlags::SAMPLED),
            (desc.storage, vk::ImageUsageFlags::STORAGE),
            (desc.copy_source, vk::ImageUsageFlags::TRANSFER_SRC),
            (desc.copy_destination, vk::ImageUsageFlags::TRANSFER_DST),
        ] {
            if enabled {
                usage |= flag;
            }
        }
        if desc.attachment {
            usage |= if matches!(desc.format, Format::Depth32Float) {
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
            } else {
                vk::ImageUsageFlags::COLOR_ATTACHMENT
            };
        }
        self.create_image(desc.size[0], desc.size[1], format(desc.format), usage)
    }
}
