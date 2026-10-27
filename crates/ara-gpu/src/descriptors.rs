//! Native-backend-independent allocation and queue descriptors.

/// Baseline image formats; backends explicitly map their native equivalents.
#[derive(Debug, Clone, Copy)]
pub enum Format {
    /// Linear RGBA8.
    Rgba8,
    /// sRGB color with linear alpha.
    Rgba8Srgb,
    /// Linear HDR RGBA32.
    Rgba32Float,
    /// Linear scRGB/HDR intermediate.
    Rgba16Float,
    /// Packed HDR10 PQ output, least-significant R channel.
    Rgb10A2,
    /// Single floating channel (IDs and diagnostics).
    R32Float,
    /// Reversed-Z floating depth.
    Depth32Float,
}
/// Buffer allocation and declared reachable uses.
pub struct BufferDesc {
    /// Nonzero byte capacity.
    pub size: u64,
    /// Coherent upload/readback memory; false selects device-local storage.
    pub host_visible: bool,
    /// Triangle/instance input for acceleration structure construction.
    pub acceleration_input: bool,
    /// Device address access, required by acceleration structure inputs.
    pub device_address: bool,
    /// Vertex input.
    pub vertex: bool,
    /// U32 index input.
    pub index: bool,
    /// Shader-readable/writable storage.
    pub storage: bool,
    /// Transfer source.
    pub copy_source: bool,
    /// Transfer destination.
    pub copy_destination: bool,
}
/// Single-level image allocation and declared reachable uses.
pub struct ImageDesc {
    /// Nonzero physical pixel extent.
    pub size: [u32; 2],
    /// Portable storage format.
    pub format: Format,
    /// Sampled image binding.
    pub sampled: bool,
    /// Writable compute image binding.
    pub storage: bool,
    /// Color/depth raster attachment, inferred from format.
    pub attachment: bool,
    /// Transfer source.
    pub copy_source: bool,
    /// Transfer destination.
    pub copy_destination: bool,
}
/// Native timestamp capabilities, without assumptions about Metal counter scope.
pub struct TimestampCaps {
    /// Significant counter bits, or zero when unsupported.
    pub valid_bits: u32,
    /// Nanoseconds represented by one counter tick.
    pub period_ns: f32,
}
/// Capabilities of one logical queue.
pub struct QueueInfo {
    /// Stable logical queue identity.
    pub id: crate::Queue,
    /// Whether the queue has a dedicated native family/command queue.
    pub dedicated: bool,
    /// Supported timestamp representation.
    pub timestamps: TimestampCaps,
}

impl Format {
    /// The ash format both backends use as their shared resource vocabulary.
    pub fn vk(self) -> ash::vk::Format {
        use ash::vk::Format as F;
        match self {
            Self::Rgba8 => F::R8G8B8A8_UNORM,
            Self::Rgba8Srgb => F::R8G8B8A8_SRGB,
            Self::Rgba32Float => F::R32G32B32A32_SFLOAT,
            Self::Rgba16Float => F::R16G16B16A16_SFLOAT,
            Self::Rgb10A2 => F::A2B10G10R10_UNORM_PACK32,
            Self::R32Float => F::R32_SFLOAT,
            Self::Depth32Float => F::D32_SFLOAT,
        }
    }
}

impl BufferDesc {
    /// Declared uses as ash buffer usage flags.
    pub fn vk_usage(&self) -> ash::vk::BufferUsageFlags {
        use ash::vk::BufferUsageFlags as U;
        [
            (self.vertex, U::VERTEX_BUFFER),
            (self.device_address, U::SHADER_DEVICE_ADDRESS),
            (
                self.acceleration_input,
                U::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            ),
            (self.index, U::INDEX_BUFFER),
            (self.storage, U::STORAGE_BUFFER),
            (self.copy_source, U::TRANSFER_SRC),
            (self.copy_destination, U::TRANSFER_DST),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .fold(U::empty(), |usage, (_, flag)| usage | flag)
    }
}

impl ImageDesc {
    /// Declared uses as ash image usage flags; attachments follow the format.
    pub fn vk_usage(&self) -> ash::vk::ImageUsageFlags {
        use ash::vk::ImageUsageFlags as U;
        let mut usage = [
            (self.sampled, U::SAMPLED),
            (self.storage, U::STORAGE),
            (self.copy_source, U::TRANSFER_SRC),
            (self.copy_destination, U::TRANSFER_DST),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .fold(U::empty(), |usage, (_, flag)| usage | flag);
        if self.attachment {
            usage |= if matches!(self.format, Format::Depth32Float) {
                U::DEPTH_STENCIL_ATTACHMENT
            } else {
                U::COLOR_ATTACHMENT
            };
        }
        usage
    }
}
