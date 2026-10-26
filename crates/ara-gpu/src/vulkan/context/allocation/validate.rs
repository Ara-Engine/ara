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
        ..
    } = *desc;
    let [width, height] = size;
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
        || mip_levels > width.max(height).ilog2() + 1
    {
        return Err(GpuError::Contract(
            "Image extent or mip count exceeds device limits".into(),
        ));
    }
    if mip_levels > 1
        && usage.intersects(
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        )
    {
        return Err(GpuError::Contract(
            "Mip chains are sampled textures, not storage or attachment images".into(),
        ));
    }
    // SAFETY: Capability query is read-only and validates this format/usage combination.
    let caps = unsafe {
        instance.get_physical_device_image_format_properties(
            physical,
            format,
            vk::ImageType::TYPE_2D,
            vk::ImageTiling::OPTIMAL,
            usage,
            vk::ImageCreateFlags::empty(),
        )
    }
    .map_err(anyhow::Error::from)?;
    if mip_levels > caps.max_mip_levels
        || width > caps.max_extent.width
        || height > caps.max_extent.height
    {
        return Err(GpuError::Contract(
            "Image shape exceeds format-specific limits".into(),
        ));
    }
    Ok(())
}
