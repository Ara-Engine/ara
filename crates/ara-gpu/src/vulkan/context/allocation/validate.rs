use super::*;
use crate::{GpuError, Result};

pub(crate) fn validate_image(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    compressed_bc: bool,
    max_dimension: u32,
    desc: &ImageDesc,
) -> Result<()> {
    let ImageDesc {
        size,
        format,
        mip_levels,
        usage,
        shape,
        ..
    } = *desc;
    let [width, height] = size;
    shape.validate(size)?;
    if shape.dimension == crate::texture::TextureDimension::D3
        && usage.intersects(
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        )
    {
        return Err(GpuError::Contract(
            "3D textures expose sampled/storage views, not raster attachments".into(),
        ));
    }
    crate::attachments::validate_usage(usage)?;
    if matches!(
        format,
        vk::Format::BC4_UNORM_BLOCK
            | vk::Format::BC5_UNORM_BLOCK
            | vk::Format::BC6H_UFLOAT_BLOCK
            | vk::Format::BC7_UNORM_BLOCK
            | vk::Format::BC7_SRGB_BLOCK
    ) && !compressed_bc
    {
        return Err(GpuError::Contract(
            "BC texture compression is unavailable on this device".into(),
        ));
    }
    if width == 0
        || height == 0
        || width > max_dimension
        || height > max_dimension
        || mip_levels == 0
        || mip_levels > width.max(height).max(shape.depth).ilog2() + 1
    {
        return Err(GpuError::Contract(
            "Image extent or mip count exceeds device limits".into(),
        ));
    }
    // SAFETY: Capability query is read-only and validates this format/usage combination.
    let caps = unsafe {
        instance.get_physical_device_image_format_properties(
            physical,
            format,
            if shape.dimension == crate::texture::TextureDimension::D3 {
                vk::ImageType::TYPE_3D
            } else {
                vk::ImageType::TYPE_2D
            },
            vk::ImageTiling::OPTIMAL,
            usage,
            // An sRGB storage image is created mutable with extended usage: its
            // UNORM alias view takes the storage writes (`storage_alias`).
            if usage.contains(vk::ImageUsageFlags::STORAGE)
                && super::image::storage_alias(format).is_some()
            {
                super::image::storage_alias_flags()
            } else {
                vk::ImageCreateFlags::empty()
            },
        )
    }
    .map_err(anyhow::Error::from)?;
    if mip_levels > caps.max_mip_levels
        || width > caps.max_extent.width
        || height > caps.max_extent.height
        || shape.depth > caps.max_extent.depth
        || shape.layers > caps.max_array_layers
        || !caps.sample_counts.contains(shape.samples.vk())
    {
        return Err(GpuError::Contract(
            "Image shape exceeds format-specific limits".into(),
        ));
    }
    Ok(())
}
