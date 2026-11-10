//! Buffer and texture allocation from the portable (ash-typed) descriptions the
//! Vulkan backend accepts. Host-visible buffers are shared storage (coherent on Apple
//! silicon); everything else is private, so uploads go through copy commands.
use super::native::{FormatInfo, format, nil, texture_usage};
use crate::{GpuError, Result};
use ash::vk;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;

/// An owned Metal buffer and its declared uses.
pub(super) struct NativeBuffer {
    pub buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    pub size: u64,
    pub usage: vk::BufferUsageFlags,
    pub host: bool,
}

/// An owned Metal texture and its declared uses.
pub(super) struct NativeImage {
    pub texture: Retained<ProtocolObject<dyn MTLTexture>>,
    pub info: FormatInfo,
    pub size: [u32; 2],
    pub levels: u32,
    pub shape: crate::texture::ImageShape,
    pub usage: vk::ImageUsageFlags,
    /// Storage-image table slot; sampled slots live on the owning record.
    pub storage: Option<crate::HeapIndex>,
}

impl NativeImage {
    /// A transient attachment in tile memory only, with no allocation to make resident.
    pub fn memoryless(&self) -> bool {
        self.usage
            .contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT)
    }
}

pub(super) fn buffer(
    device: &ProtocolObject<dyn MTLDevice>,
    size: u64,
    usage: vk::BufferUsageFlags,
    host: bool,
) -> Result<NativeBuffer> {
    if size == 0 || usage.is_empty() {
        return Err(GpuError::Contract("Empty buffer".into()));
    }
    let usage = if host {
        usage | vk::BufferUsageFlags::TRANSFER_DST
    } else {
        usage
    };
    let length = usize::try_from(size)
        .ok()
        .filter(|&length| length as u64 <= device.maxBufferLength() as u64)
        .ok_or_else(|| GpuError::Contract(format!("Buffer of {size} bytes is too large")))?;
    let storage = if host {
        MTLResourceOptions::StorageModeShared
    } else {
        MTLResourceOptions::StorageModePrivate
    };
    let buffer = device
        .newBufferWithLength_options(length, storage)
        .ok_or_else(|| nil("buffer"))?;
    Ok(NativeBuffer {
        buffer,
        size,
        usage,
        host,
    })
}

pub(super) fn image(
    device: &ProtocolObject<dyn MTLDevice>,
    size: [u32; 2],
    format_id: vk::Format,
    levels: u32,
    usage: vk::ImageUsageFlags,
) -> Result<NativeImage> {
    image_shaped(
        device,
        size,
        format_id,
        usage,
        crate::texture::ImageShape {
            mip_levels: levels,
            ..Default::default()
        },
    )
}

pub(super) fn image_shaped(
    device: &ProtocolObject<dyn MTLDevice>,
    size: [u32; 2],
    format_id: vk::Format,
    usage: vk::ImageUsageFlags,
    shape: crate::texture::ImageShape,
) -> Result<NativeImage> {
    use crate::texture::{SampleCount, TextureDimension};
    shape.validate(size)?;
    let levels = shape.mip_levels;
    let info = format(format_id)?;
    let [width, height] = size;
    let full = 32 - width.max(height).max(shape.depth).max(1).leading_zeros();
    // Like Vulkan, an extent past the reported `max_image_dimension` is an error here
    // rather than whatever the driver does with it.
    let largest = super::info::MAX_IMAGE_DIMENSION;
    if width == 0
        || height == 0
        || width > largest
        || height > largest
        || levels == 0
        || levels > full
        || usage.is_empty()
    {
        return Err(GpuError::Contract(format!(
            "Invalid image {width}x{height}, {levels} levels"
        )));
    }
    if info.block > 1
        && usage.intersects(
            !(vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC),
        )
    {
        return Err(GpuError::Contract(
            "Block-compressed images are sampled/transfer only".into(),
        ));
    }
    // SAFETY: the format is a valid 2D pixel format and the extent is nonzero.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            info.pixel,
            width as usize,
            height as usize,
            false,
        )
    };
    // SAFETY: `levels` was checked against the full mip chain of this extent.
    unsafe { descriptor.setMipmapLevelCount(levels as usize) };
    let kind = match (shape.dimension, shape.samples, shape.layers > 1) {
        (TextureDimension::D3, _, _) => MTLTextureType::Type3D,
        (_, SampleCount::Four, false) => MTLTextureType::Type2DMultisample,
        (_, SampleCount::Four, true) => MTLTextureType::Type2DMultisampleArray,
        (_, _, true) => MTLTextureType::Type2DArray,
        _ => MTLTextureType::Type2D,
    };
    if !device.supportsTextureSampleCount(shape.samples.count() as usize) {
        return Err(GpuError::Contract(
            "Unsupported texture sample count".into(),
        ));
    }
    // SAFETY: the validated shape has positive depth/layers and compatible mip/sample counts.
    unsafe {
        descriptor.setTextureType(kind);
        descriptor.setDepth(shape.depth as usize);
        descriptor.setArrayLength(shape.layers as usize);
        descriptor.setSampleCount(shape.samples.count() as usize);
    }
    descriptor.setUsage(texture_usage(usage));
    crate::attachments::validate_usage(usage)?;
    // Transient attachments live in tile memory only, with no backing allocation.
    descriptor.setStorageMode(
        if usage.contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT) {
            MTLStorageMode::Memoryless
        } else {
            MTLStorageMode::Private
        },
    );
    let texture = device
        .newTextureWithDescriptor(&descriptor)
        .ok_or_else(|| nil("texture"))?;
    Ok(NativeImage {
        texture,
        info,
        size,
        levels,
        shape,
        usage,
        storage: None,
    })
}

/// Write shared buffer storage. Callers have checked GPU completion and bounds.
pub(super) fn write(buffer: &NativeBuffer, offset: u64, bytes: &[u8]) -> Result<()> {
    check_host_range(buffer, offset, bytes.len() as u64)?;
    // SAFETY: the range lies inside this shared buffer and no GPU work that uses the
    // buffer is pending (checked by the caller against both timelines).
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            buffer
                .buffer
                .contents()
                .cast::<u8>()
                .as_ptr()
                .add(offset as usize),
            bytes.len(),
        );
    }
    Ok(())
}

/// Copy shared buffer storage out. Callers have checked GPU completion.
pub(super) fn read(buffer: &NativeBuffer) -> Result<Vec<u8>> {
    check_host_range(buffer, 0, buffer.size)?;
    // SAFETY: the whole shared allocation is readable, and every GPU write to it
    // completed before this call (checked by the caller).
    Ok(unsafe {
        std::slice::from_raw_parts(
            buffer.buffer.contents().cast::<u8>().as_ptr(),
            buffer.size as usize,
        )
    }
    .to_vec())
}

fn check_host_range(buffer: &NativeBuffer, offset: u64, len: u64) -> Result<()> {
    if !buffer.host {
        return Err(GpuError::Contract(
            "Buffer is device-local; use a host-visible staging buffer".into(),
        ));
    }
    if offset.checked_add(len).is_none_or(|end| end > buffer.size) {
        return Err(GpuError::Contract("Host access exceeds buffer size".into()));
    }
    Ok(())
}
