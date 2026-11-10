//! Portable resource ranges for explicit, batched graph dependencies.
use crate::{Barrier, GpuError, Result};
/// Part of a resource covered by a dependency. Whole is the graph's default.
#[derive(Debug, Clone, Copy, Default)]
pub enum BarrierRange {
    /// Every byte, mip and layer.
    #[default]
    Whole,
    /// A nonempty byte interval in a buffer.
    Buffer {
        /// First byte.
        offset: u64,
        /// Byte count.
        size: u64,
    },
    /// Resident image mip and array-layer intervals.
    Image {
        /// First mip.
        base_mip: u32,
        /// Number of mips.
        mip_count: u32,
        /// First array layer.
        base_layer: u32,
        /// Number of array layers.
        layer_count: u32,
    },
}
/// One graph dependency, optionally limited to part of a resource.
#[derive(Debug, Clone, Copy)]
pub struct ResourceBarrier {
    /// Accesses and optional queue handoff.
    pub barrier: Barrier,
    /// Bytes or image subresources affected. Metal conservatively synchronizes
    /// all resources at these stages; Vulkan uses this exact range.
    pub range: BarrierRange,
}
impl From<Barrier> for ResourceBarrier {
    fn from(barrier: Barrier) -> Self {
        Self {
            barrier,
            range: BarrierRange::Whole,
        }
    }
}
impl BarrierRange {
    pub(crate) fn buffer(self, capacity: u64) -> Result<(u64, u64)> {
        let (offset, size) = match self {
            Self::Whole => (0, capacity),
            Self::Buffer { offset, size } => (offset, size),
            _ => return Err(GpuError::Contract("Image range used on a buffer".into())),
        };
        if size == 0 || offset.checked_add(size).is_none_or(|end| end > capacity) {
            return Err(GpuError::Contract("Barrier exceeds buffer range".into()));
        }
        Ok((offset, size))
    }
    pub(crate) fn image(self, shape: crate::texture::ImageShape) -> Result<(u32, u32, u32, u32)> {
        let r = match self {
            Self::Whole => (0, shape.mip_levels, 0, shape.layers),
            Self::Image {
                base_mip,
                mip_count,
                base_layer,
                layer_count,
            } => (base_mip, mip_count, base_layer, layer_count),
            _ => return Err(GpuError::Contract("Buffer range used on an image".into())),
        };
        if r.1 == 0
            || r.3 == 0
            || r.0
                .checked_add(r.1)
                .is_none_or(|end| end > shape.mip_levels)
            || r.2.checked_add(r.3).is_none_or(|end| end > shape.layers)
        {
            return Err(GpuError::Contract(
                "Barrier exceeds image subresources".into(),
            ));
        }
        Ok(r)
    }
}
