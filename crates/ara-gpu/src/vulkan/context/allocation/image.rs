use super::super::resources::range;
use super::*;
use ash::vk::TaggedStructure;

pub(crate) fn pooled_image(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    desc: ImageDesc,
    mut blocks: Option<&mut super::super::memory_blocks::MemoryBlocks>,
) -> Result<ImageAllocation> {
    let mut suballocation = None;
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
        // A square layered image may be viewed as cubes (`ViewDimension::CubeArray`).
        let cube = shape.dimension != crate::texture::TextureDimension::D3
            && shape.samples == crate::texture::SampleCount::One
            && width == height
            && shape.layers >= 6;
        let mut flags = if cube {
            vk::ImageCreateFlags::CUBE_COMPATIBLE
        } else {
            vk::ImageCreateFlags::empty()
        };
        let mut info = vk::ImageCreateInfo::default();
        if alias.is_some() {
            flags |= storage_alias_flags();
            info = info.push(&mut list);
        }
        info = info.flags(flags);
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
        let mut attachment_view = None;
        let result = (|| -> Result<_> {
            let mut dedicated = vk::MemoryDedicatedRequirements::default();
            let mut req2 = vk::MemoryRequirements2::default().push(&mut dedicated);
            device.get_image_memory_requirements2(
                &vk::ImageMemoryRequirementsInfo2::default().image(handle),
                &mut req2,
            );
            let req = req2.memory_requirements;
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
            if dedicated.requires_dedicated_allocation == vk::FALSE
                && !usage.contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT)
                && let Some(blocks) = blocks.as_deref_mut()
            {
                let slice = blocks.allocate(
                    device,
                    super::super::memory_blocks::Key {
                        memory_type: index,
                        address: false,
                        class: super::super::memory_blocks::Class::Image,
                    },
                    req,
                )?;
                memory = slice.memory;
                suballocation = Some(slice);
            } else {
                let mut own = vk::MemoryDedicatedAllocateInfo::default().image(handle);
                let mut info = vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(index);
                if dedicated.requires_dedicated_allocation != vk::FALSE {
                    info = info.push(&mut own);
                }
                memory = device.allocate_memory(&info, None)?;
            }
            device.bind_image_memory(handle, memory, suballocation.map_or(0, |s| s.offset))?;
            // The image's own format cannot be stored to: its view leaves storage out.
            let shader_chain = mip_levels > 1 && usage.contains(vk::ImageUsageFlags::SAMPLED);
            let view_levels = if shader_chain { mip_levels } else { 1 };
            let view_usage = if shader_chain {
                vk::ImageUsageFlags::SAMPLED
            } else if alias.is_some() {
                usage & !vk::ImageUsageFlags::STORAGE
            } else {
                usage
            };
            let mut own = vk::ImageViewUsageCreateInfo::default().usage(view_usage);
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
                        .level_count(view_levels)
                        .layer_count(shape.layers),
                );
            if alias.is_some() || shader_chain {
                view_info = view_info.push(&mut own);
            }
            let viewable = usage.intersects(
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    | vk::ImageUsageFlags::INPUT_ATTACHMENT,
            );
            if viewable {
                view = device.create_image_view(&view_info, None)?;
            }
            if shader_chain
                && usage.intersects(
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                        | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                )
            {
                let mut attachment_usage = vk::ImageViewUsageCreateInfo::default().usage(
                    usage
                        & (vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT),
                );
                attachment_view = Some(
                    device.create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(handle)
                            .view_type(if shape.layers > 1 {
                                vk::ImageViewType::TYPE_2D_ARRAY
                            } else {
                                vk::ImageViewType::TYPE_2D
                            })
                            .format(format)
                            .subresource_range(
                                range(aspect).level_count(1).layer_count(shape.layers),
                            )
                            .push(&mut attachment_usage),
                        None,
                    )?,
                );
            }
            if usage.contains(vk::ImageUsageFlags::STORAGE) {
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
                            .format(alias.unwrap_or(format))
                            .subresource_range(
                                range(aspect).level_count(1).layer_count(shape.layers),
                            )
                            .push(&mut storage),
                        None,
                    )?,
                );
            }
            Ok(ImageAllocation {
                handle,
                memory,
                suballocation,
                view,
                storage_view,
                attachment_view,
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
                if let Some(attachment) = attachment_view {
                    device.destroy_image_view(attachment, None);
                }
                device.destroy_image(handle, None);
                if let Some(slice) = suballocation {
                    blocks.expect("pooled image has an owner").release(slice);
                } else if memory != vk::DeviceMemory::null() {
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
