//! Immutable samplers published into the global heap. Slots 0..3 retain the
//! material, environment and temporal defaults; additional descriptions are cached
//! for the device lifetime, so changing a sampler never overwrites in-flight state.
use crate::{GpuError, Result};

/// Capacity of each sampler table; regular samplers include three engine defaults.
pub const SAMPLER_CAPACITY: u32 = 64;
/// Texel or mip interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    /// Select one texel or mip.
    Nearest,
    /// Interpolate adjacent texels or mips.
    #[default]
    Linear,
}
/// Coordinates outside the normalized texture extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AddressMode {
    /// Wrap every integer interval.
    Repeat,
    /// Wrap, alternating orientation each interval.
    MirrorRepeat,
    /// Clamp to the nearest edge texel.
    #[default]
    ClampToEdge,
}
/// Depth comparison, shared by raster state and comparison samplers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum CompareOp {
    /// Never pass.
    Never,
    /// Incoming depth is less than stored depth.
    Less,
    /// Equal depths.
    Equal,
    /// Less than or equal.
    LessEqual,
    /// Greater than stored depth.
    Greater,
    /// Unequal depths.
    NotEqual,
    /// Greater than or equal (reversed Z).
    #[default]
    GreaterEqual,
    /// Always pass.
    Always,
}
impl CompareOp {
    pub(crate) fn vk(self) -> ash::vk::CompareOp {
        use ash::vk::CompareOp as C;
        match self {
            Self::Never => C::NEVER,
            Self::Less => C::LESS,
            Self::Equal => C::EQUAL,
            Self::LessEqual => C::LESS_OR_EQUAL,
            Self::Greater => C::GREATER,
            Self::NotEqual => C::NOT_EQUAL,
            Self::GreaterEqual => C::GREATER_OR_EQUAL,
            Self::Always => C::ALWAYS,
        }
    }
}
/// Portable filtering and addressing. Anisotropy requires linear filters and is
/// clamped to the backend's reported hardware limit when unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SamplerDesc {
    /// Minification filter.
    pub min: Filter,
    /// Magnification filter.
    pub mag: Filter,
    /// Mip interpolation.
    pub mip: Filter,
    /// U, V and W coordinate addressing.
    pub address: [AddressMode; 3],
    /// Optional depth comparison. Some publishes in the comparison table
    /// (Vulkan binding 6 / DXIL s0 space5); None uses binding 2 / s0 space0.
    pub compare: Option<CompareOp>,
    /// Maximum anisotropy, 1..=16; one disables it.
    pub anisotropy: u8,
}
impl Default for SamplerDesc {
    fn default() -> Self {
        Self {
            min: Filter::Linear,
            mag: Filter::Linear,
            mip: Filter::Linear,
            address: [AddressMode::ClampToEdge; 3],
            compare: None,
            anisotropy: 1,
        }
    }
}
impl SamplerDesc {
    pub(crate) fn validate(self) -> Result<()> {
        if !(1..=16).contains(&self.anisotropy)
            || (self.anisotropy > 1 && [self.min, self.mag, self.mip].contains(&Filter::Nearest))
        {
            return Err(GpuError::Contract(
                "Anisotropy must be 1..=16 and requires linear filters".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn defaults() -> [Self; 3] {
        use AddressMode::{ClampToEdge as C, Repeat as R};
        [
            Self {
                address: [R, R, C],
                anisotropy: 16,
                ..Self::default()
            },
            Self {
                address: [R, C, C],
                ..Self::default()
            },
            Self::default(),
        ]
    }
}
