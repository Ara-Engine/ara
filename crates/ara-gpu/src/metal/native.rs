//! Lowering of the portable (ash-typed) resource vocabulary to Metal, plus error
//! conversion. Consumers keep passing `vk::Format`/usage flags; only this backend
//! knows the Metal equivalents.
use crate::{GpuError, Result};
use ash::vk;
use objc2::rc::Retained;
use objc2_foundation::NSError;
use objc2_metal::{MTLPixelFormat, MTLTextureUsage};

/// A backend failure carrying the Metal error description.
pub(super) fn ns_error(context: &str, error: &Retained<NSError>) -> GpuError {
    GpuError::Backend(anyhow::anyhow!(
        "{context}: {}",
        error.localizedDescription()
    ))
}

/// A backend failure for a Metal factory method that returned nil.
pub(super) fn nil(context: &str) -> GpuError {
    GpuError::Backend(anyhow::anyhow!("{context} returned nil"))
}

/// Metal pixel format, block footprint and bytes per block for an engine format.
/// Uncompressed formats use 1x1 blocks.
pub(super) struct FormatInfo {
    pub pixel: MTLPixelFormat,
    pub block: u32,
    pub block_bytes: u32,
    pub depth: bool,
}

/// Map every `vk::Format` the engine allocates; anything else is a contract error.
pub(super) fn format(format: vk::Format) -> Result<FormatInfo> {
    use MTLPixelFormat as P;
    let (pixel, block, block_bytes, depth) = match format {
        vk::Format::R8G8B8A8_UNORM => (P::RGBA8Unorm, 1, 4, false),
        vk::Format::R8G8B8A8_SRGB => (P::RGBA8Unorm_sRGB, 1, 4, false),
        vk::Format::B8G8R8A8_UNORM => (P::BGRA8Unorm, 1, 4, false),
        vk::Format::B8G8R8A8_SRGB => (P::BGRA8Unorm_sRGB, 1, 4, false),
        vk::Format::R16_SFLOAT => (P::R16Float, 1, 2, false),
        vk::Format::R16G16_SFLOAT => (P::RG16Float, 1, 4, false),
        vk::Format::R16G16_UNORM => (P::RG16Unorm, 1, 4, false),
        vk::Format::R16G16B16A16_SFLOAT => (P::RGBA16Float, 1, 8, false),
        vk::Format::R32_SFLOAT => (P::R32Float, 1, 4, false),
        vk::Format::R32G32_SFLOAT => (P::RG32Float, 1, 8, false),
        vk::Format::R32G32B32A32_SFLOAT => (P::RGBA32Float, 1, 16, false),
        vk::Format::A2B10G10R10_UNORM_PACK32 => (P::RGB10A2Unorm, 1, 4, false),
        vk::Format::A2R10G10B10_UNORM_PACK32 => (P::BGR10A2Unorm, 1, 4, false),
        vk::Format::B10G11R11_UFLOAT_PACK32 => (P::RG11B10Float, 1, 4, false),
        vk::Format::D32_SFLOAT => (P::Depth32Float, 1, 4, true),
        vk::Format::BC4_UNORM_BLOCK => (P::BC4_RUnorm, 4, 8, false),
        vk::Format::BC5_UNORM_BLOCK => (P::BC5_RGUnorm, 4, 16, false),
        vk::Format::BC6H_UFLOAT_BLOCK => (P::BC6H_RGBUfloat, 4, 16, false),
        vk::Format::BC7_UNORM_BLOCK => (P::BC7_RGBAUnorm, 4, 16, false),
        vk::Format::BC7_SRGB_BLOCK => (P::BC7_RGBAUnorm_sRGB, 4, 16, false),
        other => {
            return Err(GpuError::Contract(format!(
                "Image format {other:?} has no Metal mapping"
            )));
        }
    };
    Ok(FormatInfo {
        pixel,
        block,
        block_bytes,
        depth,
    })
}

/// Raster attachment formats arrive as raw Vulkan IDs in cooked render state.
pub(super) fn raw_format(raw: i32) -> Result<MTLPixelFormat> {
    Ok(format(vk::Format::from_raw(raw))?.pixel)
}

/// Texture usage implied by the portable image usage flags.
pub(super) fn texture_usage(usage: vk::ImageUsageFlags) -> MTLTextureUsage {
    let mut metal = MTLTextureUsage::Unknown;
    if usage.intersects(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::INPUT_ATTACHMENT) {
        metal |= MTLTextureUsage::ShaderRead;
    }
    if usage.contains(vk::ImageUsageFlags::STORAGE) {
        metal |= MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
    }
    if usage.intersects(
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
    ) {
        metal |= MTLTextureUsage::RenderTarget;
    }
    metal
}

/// Bytes of one tightly packed mip level.
pub(super) fn level_bytes(info: &FormatInfo, size: [u32; 2], level: u32) -> (u64, u64) {
    let width = (size[0] >> level).max(1);
    let height = (size[1] >> level).max(1);
    let row = u64::from(width.div_ceil(info.block)) * u64::from(info.block_bytes);
    (row, row * u64::from(height.div_ceil(info.block)))
}
