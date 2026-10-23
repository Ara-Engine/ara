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
