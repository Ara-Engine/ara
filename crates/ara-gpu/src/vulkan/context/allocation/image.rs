use super::super::resources::range;
use super::*;
use ash::vk::TaggedStructure;

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
        shape,
    } = desc;
    let alias = usage
        .contains(vk::ImageUsageFlags::STORAGE)
        .then(|| storage_alias(format))
        .flatten();
    let formats = [format, alias.unwrap_or(format)];
    let mut list = vk::ImageFormatListCreateInfo::default().view_formats(&formats);
    // SAFETY: Memory/view match queried image requirements. Gpu owns all three.
    unsafe {
        let mut info = vk::ImageCreateInfo::default();
        if alias.is_some() {
            info = info.flags(storage_alias_flags()).push(&mut list);
        }
        let handle = device.create_image(
            &info
                .image_type(if shape.dimension == crate::texture::TextureDimension::D3 {
                    vk::ImageType::TYPE_3D
                } else {
                    vk::ImageType::TYPE_2D
                })
                .format(format)
                .extent(vk::Extent3D {
                    width,
                    height,
                    depth: shape.depth,
                })
                .mip_levels(mip_levels)
                .array_layers(shape.layers)
                .samples(shape.samples.vk())
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        let mut memory = vk::DeviceMemory::null();
        let mut view = vk::ImageView::null();
        let mut storage_view = None;
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
            // The image's own format cannot be stored to: its view leaves storage out.
            let mut own = vk::ImageViewUsageCreateInfo::default()
                .usage(usage & !vk::ImageUsageFlags::STORAGE);
            let mut view_info = vk::ImageViewCreateInfo::default()
                .image(handle)
                .view_type(if shape.dimension == crate::texture::TextureDimension::D3 {
                    vk::ImageViewType::TYPE_3D
                } else if shape.layers > 1 {
                    vk::ImageViewType::TYPE_2D_ARRAY
                } else {
                    vk::ImageViewType::TYPE_2D
                })
                .format(format)
                .subresource_range(
                    range(aspect)
                        .level_count(mip_levels)
                        .layer_count(shape.layers),
                );
            if alias.is_some() {
                view_info = view_info.push(&mut own);
            }
            view = device.create_image_view(&view_info, None)?;
            if let Some(alias) = alias {
                let mut storage =
                    vk::ImageViewUsageCreateInfo::default().usage(vk::ImageUsageFlags::STORAGE);
                storage_view = Some(
                    device.create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(handle)
                            .view_type(if shape.dimension == crate::texture::TextureDimension::D3 {
                                vk::ImageViewType::TYPE_3D
                            } else if shape.layers > 1 {
                                vk::ImageViewType::TYPE_2D_ARRAY
                            } else {
                                vk::ImageViewType::TYPE_2D
                            })
                            .format(alias)
                            .subresource_range(
                                range(aspect)
                                    .level_count(mip_levels)
                                    .layer_count(shape.layers),
                            )
                            .push(&mut storage),
                        None,
                    )?,
                );
            }
            Ok(ImageAllocation {
                handle,
                memory,
                view,
                storage_view,
                width,
                height,
                mip_levels,
                shape,
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
                if let Some(storage) = storage_view {
                    device.destroy_image_view(storage, None);
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

/// The format storage writes use for a storage image whose own format shaders cannot
/// write (sRGB colour: compute passes encode it themselves), or `None`.
pub(crate) fn storage_alias(format: vk::Format) -> Option<vk::Format> {
    match format {
        vk::Format::R8G8B8A8_SRGB => Some(vk::Format::R8G8B8A8_UNORM),
        vk::Format::B8G8R8A8_SRGB => Some(vk::Format::B8G8R8A8_UNORM),
        _ => None,
    }
}

/// Creation flags of an image with a storage alias: views may reinterpret its
/// format, and the storage usage need only hold for the alias.
pub(crate) fn storage_alias_flags() -> vk::ImageCreateFlags {
    vk::ImageCreateFlags::MUTABLE_FORMAT | vk::ImageCreateFlags::EXTENDED_USAGE
}
