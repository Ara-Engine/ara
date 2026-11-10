//! Portable raster pipeline and vertex input state. Defaults retain Ara's legacy
//! no-cull, reversed-Z, single-sample behavior.
use crate::{GpuError, Result};
use serde::{Deserialize, Serialize};

/// Portable simultaneous vertex streams.
pub const MAX_VERTEX_BUFFERS: usize = 8;
/// Vertex attribute locations, translated to converter stage-in locations on Metal.
pub const MAX_VERTEX_ATTRIBUTES: u32 = 16;
/// Faces removed before rasterization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CullMode {
    /// Keep both sides.
    #[default]
    None,
    /// Remove front faces.
    Front,
    /// Remove back faces.
    Back,
}
/// Primitive front face winding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FrontFace {
    /// Counter-clockwise triangles face front.
    #[default]
    CounterClockwise,
    /// Clockwise triangles face front.
    Clockwise,
}
/// Rate of advancement through a vertex stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VertexStep {
    /// One record per vertex.
    #[default]
    Vertex,
    /// One record per instance.
    Instance,
}
/// Attribute storage interpreted by vertex fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VertexFormat {
    /// One 32-bit float.
    Float32,
    /// Two 32-bit floats.
    Float32x2,
    /// Three 32-bit floats.
    Float32x3,
    /// Four 32-bit floats.
    Float32x4,
    /// One unsigned 32-bit integer.
    Uint32,
    /// Two unsigned 32-bit integers.
    Uint32x2,
    /// Three unsigned 32-bit integers.
    Uint32x3,
    /// Four unsigned 32-bit integers.
    Uint32x4,
    /// Four normalized unsigned bytes.
    Unorm8x4,
}
impl VertexFormat {
    pub(crate) fn size(self) -> u32 {
        match self {
            Self::Float32 | Self::Uint32 | Self::Unorm8x4 => 4,
            Self::Float32x2 | Self::Uint32x2 => 8,
            Self::Float32x3 | Self::Uint32x3 => 12,
            _ => 16,
        }
    }
    pub(crate) fn vk(self) -> ash::vk::Format {
        use ash::vk::Format as F;
        match self {
            Self::Float32 => F::R32_SFLOAT,
            Self::Float32x2 => F::R32G32_SFLOAT,
            Self::Float32x3 => F::R32G32B32_SFLOAT,
            Self::Float32x4 => F::R32G32B32A32_SFLOAT,
            Self::Uint32 => F::R32_UINT,
            Self::Uint32x2 => F::R32G32_UINT,
            Self::Uint32x3 => F::R32G32B32_UINT,
            Self::Uint32x4 => F::R32G32B32A32_UINT,
            Self::Unorm8x4 => F::R8G8B8A8_UNORM,
        }
    }
}
/// One attribute inside a stream record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexAttribute {
    /// Shader input location, starting at zero.
    pub location: u32,
    /// Native storage interpretation.
    pub format: VertexFormat,
    /// Byte offset in the record.
    pub offset: u32,
}
/// One vertex-buffer binding in pipeline order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexLayout {
    /// Record stride in bytes, divisible by four.
    pub stride: u32,
    /// Per-vertex or per-instance fetch.
    pub step: VertexStep,
    /// Attributes sourced from this stream.
    pub attributes: Vec<VertexAttribute>,
}
/// Source or destination contribution to blending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendFactor {
    /// No contribution.
    Zero,
    /// Full contribution.
    One,
    /// Source color channels.
    SrcColor,
    /// Complement of source color.
    OneMinusSrcColor,
    /// Destination color channels.
    DstColor,
    /// Complement of destination color.
    OneMinusDstColor,
    /// Source alpha.
    SrcAlpha,
    /// Complement of source alpha.
    OneMinusSrcAlpha,
    /// Destination alpha.
    DstAlpha,
    /// Complement of destination alpha.
    OneMinusDstAlpha,
}
/// Combination of weighted source and destination values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendOp {
    /// Sum the contributions.
    Add,
    /// Source minus destination.
    Subtract,
    /// Destination minus source.
    ReverseSubtract,
    /// Smaller unweighted value.
    Min,
    /// Larger unweighted value.
    Max,
}
/// One color or alpha blend equation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BlendComponent {
    /// Source contribution.
    pub source: BlendFactor,
    /// Destination contribution.
    pub destination: BlendFactor,
    /// Equation.
    pub operation: BlendOp,
}
/// Per-attachment blending; `None` disables blending on that attachment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BlendState {
    /// RGB equation.
    pub color: BlendComponent,
    /// Alpha equation.
    pub alpha: BlendComponent,
}
/// Raster options beyond the legacy cooked state. Missing fields keep defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RasterState {
    /// Native sample count.
    pub samples: crate::texture::SampleCount,
    /// Depth comparison when depth testing is enabled.
    pub depth_compare: crate::sampler::CompareOp,
    /// Constant depth bias in the backend's depth units.
    pub depth_bias: f32,
    /// Bias scaled by the polygon depth slope.
    pub depth_bias_slope: f32,
    /// Faces to cull.
    pub cull: CullMode,
    /// Front-facing winding.
    pub front_face: FrontFace,
    /// Explicit vertex streams. Empty uses `RenderState::vertex_stride`'s legacy ABI.
    pub vertex_layouts: Vec<VertexLayout>,
    /// Per-color-target blend equations; empty retains `RenderState::blend`.
    pub blends: Vec<Option<BlendState>>,
}
impl RasterState {
    pub(crate) fn validate(&self) -> Result<()> {
        let mut seen = 0u32;
        if !self.depth_bias.is_finite()
            || !self.depth_bias_slope.is_finite()
            || self.vertex_layouts.len() > MAX_VERTEX_BUFFERS
        {
            return Err(GpuError::Contract("Invalid raster state".into()));
        }
        for layout in &self.vertex_layouts {
            if layout.stride == 0
                || layout.stride > 2048
                || !layout.stride.is_multiple_of(4)
                || layout.attributes.is_empty()
            {
                return Err(GpuError::Contract(
                    "Invalid vertex stream stride or attributes".into(),
                ));
            }
            for a in &layout.attributes {
                if a.location >= MAX_VERTEX_ATTRIBUTES
                    || seen & (1 << a.location) != 0
                    || !a.offset.is_multiple_of(4)
                    || a.offset
                        .checked_add(a.format.size())
                        .is_none_or(|end| end > layout.stride)
                {
                    return Err(GpuError::Contract(
                        "Invalid vertex attribute location, offset or format".into(),
                    ));
                }
                seen |= 1 << a.location;
            }
        }
        Ok(())
    }
}
/// One buffer and starting byte offset, matching its pipeline's stream layout.
pub struct VertexBinding {
    /// Vertex buffer resource.
    pub buffer: crate::Buffer,
    /// Four-byte-aligned stream offset.
    pub offset: u64,
}
