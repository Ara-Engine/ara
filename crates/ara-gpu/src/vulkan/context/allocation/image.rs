use super::super::resources::range;
use super::*;

pub(crate) fn image(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    desc: ImageDesc,
) -> Result<ImageAllocation> {
    let ImageDesc {
        size: [width, height],
        format,
        mip_levels,
        usage,
        aspect,
    } = desc;
    // SAFETY: Memory/view match queried image requirements. Gpu owns all three.
    unsafe {
        let handle = device.create_image(
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
            let req = device.get_image_memory_requirements(handle);
            let device_local = || {
                memory_type(
                    memory_properties,
                    req.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
            };
            // Transient attachments prefer memory that tile-based GPUs never commit;
            // desktop GPUs have none and use ordinary device-local memory.
            let index = if usage.contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT) {
                memory_type(
                    memory_properties,
                    req.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL
                        | vk::MemoryPropertyFlags::LAZILY_ALLOCATED,
                )
                .or_else(|_| device_local())?
            } else {
                device_local()?
            };
            memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(index),
                None,
            )?;
            device.bind_image_memory(handle, memory, 0)?;
            view = device.create_image_view(
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
                    device.destroy_image_view(view, None);
                }
                device.destroy_image(handle, None);
                if memory != vk::DeviceMemory::null() {
                    device.free_memory(memory, None);
                }
                return Err(error);
            }
        };
        Ok(image)
    }
}
