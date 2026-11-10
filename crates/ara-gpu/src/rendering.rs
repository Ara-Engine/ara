//! Raster attachment subresources, independent of shader-view heap slots.
use crate::{
    GpuError, Image, Result,
    commands::{Attachment, Command},
    texture::{ImageShape, TextureDimension},
};
/// One 2D mip and array slice rendered or resolved into. The parent is the graph identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderTarget {
    /// Parent image.
    pub image: Image,
    /// Resident mip level.
    pub mip: u32,
    /// Array slice; zero for a plain 2D image.
    pub layer: u32,
}
impl From<Image> for RenderTarget {
    fn from(image: Image) -> Self {
        Self {
            image,
            mip: 0,
            layer: 0,
        }
    }
}
impl RenderTarget {
    pub(crate) fn extent(self, shape: ImageShape, size: [u32; 2]) -> Result<[u32; 2]> {
        if shape.dimension != TextureDimension::D2
            || self.mip >= shape.mip_levels
            || self.layer >= shape.layers
        {
            return Err(GpuError::Contract(
                "Raster target mip/layer exceeds a 2D image".into(),
            ));
        }
        let [w, h, _] = shape.extent(size, self.mip);
        Ok([w, h])
    }
}
/// Color or depth output with a selected mip/slice and optional clear.
#[derive(Clone, Copy)]
pub struct RenderAttachment {
    /// Target subresource.
    pub target: RenderTarget,
    /// Clear color, or depth in component zero; None preserves the selected subresource.
    pub clear: Option<[f32; 4]>,
}
impl From<Attachment> for RenderAttachment {
    fn from(a: Attachment) -> Self {
        Self {
            target: a.image.into(),
            clear: a.clear,
        }
    }
}
impl Command {
    pub(crate) fn color_attachments(
        &self,
    ) -> Option<impl ExactSizeIterator<Item = RenderAttachment> + Clone + '_> {
        let count = match self {
            Self::BeginRendering { colors, .. } | Self::BeginRenderingResolved { colors, .. } => {
                colors.len()
            }
            Self::BeginRenderingSubresources { colors, .. } => colors.len(),
            _ => return None,
        };
        Some((0..count).map(|i| match self {
            Self::BeginRendering { colors, .. } | Self::BeginRenderingResolved { colors, .. } => {
                colors[i].into()
            }
            Self::BeginRenderingSubresources { colors, .. } => colors[i],
            _ => unreachable!("only rendering commands create an attachment iterator"),
        }))
    }
    pub(crate) fn depth_attachment(&self) -> Option<RenderAttachment> {
        match self {
            Self::BeginRendering { depth, .. } | Self::BeginRenderingResolved { depth, .. } => {
                depth.map(Into::into)
            }
            Self::BeginRenderingSubresources { depth, .. } => *depth,
            _ => None,
        }
    }
    pub(crate) fn resolve_count(&self) -> Option<usize> {
        match self {
            Self::BeginRenderingResolved { resolves, .. } => Some(resolves.len()),
            Self::BeginRenderingSubresources { resolves, .. } if !resolves.is_empty() => {
                Some(resolves.len())
            }
            _ => None,
        }
    }
    pub(crate) fn resolve_targets(&self) -> impl Iterator<Item = Option<RenderTarget>> + '_ {
        let count = self.resolve_count().unwrap_or(0);
        (0..count).map(|i| match self {
            Self::BeginRenderingResolved { resolves, .. } => resolves[i].map(Into::into),
            Self::BeginRenderingSubresources { resolves, .. } => resolves[i],
            _ => None,
        })
    }
}
