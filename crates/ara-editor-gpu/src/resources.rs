use anyhow::{Context, Result, ensure};
use ara_gpu::{vk, vulkan::Context as Gpu};
use std::ptr::NonNull;

pub(super) struct Memory {
    pub device: ash::Device,
    pub properties: vk::PhysicalDeviceMemoryProperties,
}
impl Memory {
    pub fn new(gpu: &Gpu) -> Self {
        // SAFETY: Physical device and instance are live throughout editor GPU construction.
        let properties = unsafe {
            gpu.instance
                .get_physical_device_memory_properties(gpu.physical)
        };
        Self {
            device: gpu.device.clone(),
            properties,
        }
    }
    fn kind(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32> {
        (0..self.properties.memory_type_count)
            .find(|i| {
                bits & (1 << i) != 0
                    && self.properties.memory_types[*i as usize]
                        .property_flags
                        .contains(flags)
            })
            .context("No UI memory type")
    }
    pub fn buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> Result<Buffer> {
        let mut b = Buffer {
            device: self.device.clone(),
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: None,
            size,
        };
        // SAFETY: Allocations use queried requirements. The returned RAII object cleans partial creation.
        unsafe {
            b.buffer = self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let requirements = self.device.get_buffer_memory_requirements(b.buffer);
            b.memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(self.kind(
                        requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )?),
                None,
            )?;
            self.device.bind_buffer_memory(b.buffer, b.memory, 0)?;
            b.mapped = NonNull::new(self.device.map_memory(
                b.memory,
                0,
                size,
                vk::MemoryMapFlags::empty(),
            )?);
        }
        ensure!(b.mapped.is_some(), "Vulkan returned a null mapping");
        Ok(b)
    }
    pub fn image(&self, size: [u32; 2], usage: vk::ImageUsageFlags) -> Result<Image> {
        let mut image = Image {
            device: self.device.clone(),
            image: vk::Image::null(),
            view: vk::ImageView::null(),
            memory: vk::DeviceMemory::null(),
            size,
        };
        // SAFETY: Single-mip RGBA8 image/view match the allocated memory and are RAII-owned.
        unsafe {
            image.image = self.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width: size[0],
                        height: size[1],
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
            let requirements = self.device.get_image_memory_requirements(image.image);
            image.memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(self.kind(
                        requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )?),
                None,
            )?;
            self.device
                .bind_image_memory(image.image, image.memory, 0)?;
            image.view = self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(range()),
                None,
            )?;
        }
        Ok(image)
    }
}
pub(super) struct Buffer {
    device: ash::Device,
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: Option<NonNull<std::ffi::c_void>>,
    pub size: u64,
}
impl Buffer {
    pub fn write(&self, data: &[u8]) -> Result<()> {
        ensure!(data.len() as u64 <= self.size, "UI buffer overflow");
        let p = self.mapped.context("unmapped UI buffer")?;
        // SAFETY: The coherent mapping covers size, the copy is bounded, and submissions are completed before host writes.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), p.as_ptr().cast(), data.len());
        }
        Ok(())
    }
    pub fn read(&self, bytes: usize) -> Result<Vec<u8>> {
        ensure!(bytes as u64 <= self.size, "UI readback overflow");
        let p = self.mapped.context("unmapped readback")?;
        // SAFETY: A fence and HOST_READ barrier precede readback; this mapping owns the requested initialized bytes.
        Ok(unsafe { std::slice::from_raw_parts(p.as_ptr().cast(), bytes).to_vec() })
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: UI owner completes its queue before replacing or dropping allocations.
        unsafe {
            if self.mapped.is_some() {
                self.device.unmap_memory(self.memory);
            }
            if self.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.buffer, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}
pub(super) struct Image {
    device: ash::Device,
    pub image: vk::Image,
    pub view: vk::ImageView,
    memory: vk::DeviceMemory,
    pub size: [u32; 2],
}
impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: UI owner completes its queue; descriptors are replaced before these resources are reused.
        unsafe {
            if self.view != vk::ImageView::null() {
                self.device.destroy_image_view(self.view, None);
            }
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}
pub(super) fn range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}
pub(super) fn barrier(
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
    src: (vk::PipelineStageFlags2, vk::AccessFlags2),
    dst: (vk::PipelineStageFlags2, vk::AccessFlags2),
) -> vk::ImageMemoryBarrier2<'static> {
    ara_gpu::vulkan::image_barrier(image, vk::ImageAspectFlags::COLOR, old, new, src, dst)
}
