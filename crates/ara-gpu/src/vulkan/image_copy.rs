//! Shared byte footprints and regions for native mip uploads/readbacks.
use super::*;

pub(super) fn block(format: vk::Format) -> Result<(u32, u64)> {
    Ok(match format {
        vk::Format::BC7_UNORM_BLOCK
        | vk::Format::BC7_SRGB_BLOCK
        | vk::Format::BC5_UNORM_BLOCK
        | vk::Format::BC6H_UFLOAT_BLOCK => (4, 16),
        vk::Format::BC4_UNORM_BLOCK => (4, 8),
        vk::Format::R32G32B32A32_SFLOAT | vk::Format::R32G32B32A32_UINT => (1, 16),
        vk::Format::R16G16B16A16_SFLOAT | vk::Format::R32G32_UINT => (1, 8),
        vk::Format::A2B10G10R10_UNORM_PACK32
        | vk::Format::R16G16_SFLOAT
        | vk::Format::R8G8B8A8_UNORM
        | vk::Format::R8G8B8A8_SRGB
        | vk::Format::D32_SFLOAT
        | vk::Format::R32_SFLOAT
        | vk::Format::R32_UINT => (1, 4),
        vk::Format::R16_SFLOAT | vk::Format::D16_UNORM => (1, 2),
        _ => return Err(GpuError::Contract("Unsupported image copy format".into())),
    })
}

fn level_bytes(size: [u32; 2], block: u32, bytes: u64) -> u64 {
    u64::from(size[0].div_ceil(block)) * u64::from(size[1].div_ceil(block)) * bytes
}

pub(super) fn copy_bytes(image: ImageAllocation) -> Result<u64> {
    let (block, bytes) = block(image.format)?;
    Ok((0..image.mip_levels)
        .map(|mip| {
            level_bytes(
                [image.width, image.height].map(|v| (v >> mip).max(1)),
                block,
                bytes,
            ) * u64::from((image.shape.depth >> mip).max(1))
                * u64::from(image.shape.layers)
        })
        .sum())
}

/// Keep the existing single-level transfer path allocation-free.
pub(super) enum CopyRegions {
    Single([vk::BufferImageCopy; 1]),
    Mips(Vec<vk::BufferImageCopy>),
}

impl AsRef<[vk::BufferImageCopy]> for CopyRegions {
    fn as_ref(&self) -> &[vk::BufferImageCopy] {
        match self {
            Self::Single(region) => region,
            Self::Mips(regions) => regions,
        }
    }
}

fn region(image: ImageAllocation, mip: u32, offset: u64) -> vk::BufferImageCopy {
    let [width, height] = [image.width, image.height].map(|v| (v >> mip).max(1));
    vk::BufferImageCopy::default()
        .buffer_offset(offset)
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(image.aspect)
                .mip_level(mip)
                .layer_count(image.shape.layers),
        )
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: (image.shape.depth >> mip).max(1),
        })
}

pub(super) fn copy_regions(image: ImageAllocation) -> Result<CopyRegions> {
    if image.mip_levels == 1 {
        return Ok(CopyRegions::Single([region(image, 0, 0)]));
    }
    let (block, bytes) = block(image.format)?;
    let mut offset = 0;
    let mut regions = Vec::with_capacity(image.mip_levels as usize);
    for mip in 0..image.mip_levels {
        let [width, height] = [image.width, image.height].map(|v| (v >> mip).max(1));
        regions.push(region(image, mip, offset));
        offset += level_bytes([width, height], block, bytes)
            * u64::from((image.shape.depth >> mip).max(1))
            * u64::from(image.shape.layers);
    }
    Ok(CopyRegions::Mips(regions))
}

pub(super) fn subresource(
    image: ImageAllocation,
    r: crate::texture::ImageRegion,
) -> Result<vk::ImageSubresourceLayers> {
    let (block, bytes) = block(image.format)?;
    image
        .shape
        .region([image.width, image.height], r, block, bytes)?;
    Ok(vk::ImageSubresourceLayers::default()
        .aspect_mask(image.aspect)
        .mip_level(r.mip)
        .base_array_layer(r.base_layer)
        .layer_count(r.layer_count))
}
pub(super) fn buffer_region(
    image: ImageAllocation,
    r: crate::texture::ImageRegion,
    offset: u64,
    capacity: u64,
) -> Result<vk::BufferImageCopy> {
    let layers = subresource(image, r)?;
    let (block, bytes) = block(image.format)?;
    let (_, total) = image
        .shape
        .region([image.width, image.height], r, block, bytes)?;
    if !offset.is_multiple_of(4)
        || !offset.is_multiple_of(bytes)
        || offset.checked_add(total).is_none_or(|end| end > capacity)
    {
        return Err(GpuError::Contract(
            "Image copy exceeds buffer or has unaligned offset".into(),
        ));
    }
    Ok(vk::BufferImageCopy::default()
        .buffer_offset(offset)
        .image_subresource(layers)
        .image_offset(vk::Offset3D {
            x: r.origin[0] as i32,
            y: r.origin[1] as i32,
            z: r.origin[2] as i32,
        })
        .image_extent(vk::Extent3D {
            width: r.extent[0],
            height: r.extent[1],
            depth: r.extent[2],
        }))
}
