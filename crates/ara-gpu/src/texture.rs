//! Texture dimensions and checked subresources shared by native backends.
use crate::{GpuError, Result};

/// Texture allocation dimensionality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextureDimension {
    /// Two-dimensional image, optionally with array layers.
    #[default]
    D2,
    /// Volume texture; depth shrinks with mip level.
    D3,
}
/// Native raster sample count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SampleCount {
    /// Single sample.
    #[default]
    One,
    /// Four samples, resolved to a single-sample image.
    Four,
}
impl SampleCount {
    /// Number of samples per texel.
    pub fn count(self) -> u32 {
        match self {
            Self::One => 1,
            Self::Four => 4,
        }
    }
    pub(crate) fn vk(self) -> ash::vk::SampleCountFlags {
        ash::vk::SampleCountFlags::from_raw(self.count())
    }
}
/// Allocation shape. Default is one two-dimensional layer, mip and sample.
#[derive(Debug, Clone, Copy)]
pub struct ImageShape {
    /// Two-dimensional or volume texture.
    pub dimension: TextureDimension,
    /// Volume depth (one for two-dimensional textures).
    pub depth: u32,
    /// Array layers (one for volume textures).
    pub layers: u32,
    /// Allocated mip levels, including mip zero.
    pub mip_levels: u32,
    /// Raster samples per texel.
    pub samples: SampleCount,
    /// Attachment-only transient storage (tile memory where supported).
    pub transient: bool,
}
impl Default for ImageShape {
    fn default() -> Self {
        Self {
            dimension: TextureDimension::D2,
            depth: 1,
            layers: 1,
            mip_levels: 1,
            samples: SampleCount::One,
            transient: false,
        }
    }
}
/// Complete portable allocation; existing `ImageDesc` remains the simple 2D form.
pub struct TextureDesc {
    /// Format, two-dimensional extent and declared uses.
    pub image: crate::descriptors::ImageDesc,
    /// Depth, layers, mip and sample counts.
    pub shape: ImageShape,
}
impl TextureDesc {
    /// Buffer bytes for a whole-image transfer, mip zero first. Pixels are packed
    /// inside each mip; depth mips start on four-byte boundaries. Padding is untouched.
    pub fn copy_byte_len(&self) -> Result<u64> {
        self.shape.validate(self.image.size)?;
        if self.shape.samples != SampleCount::One {
            return Err(GpuError::Contract(
                "Resolve multisample images before buffer copies".into(),
            ));
        }
        let bytes = u64::from(self.image.format.texel_bytes());
        let align = if matches!(
            self.image.format,
            crate::descriptors::Format::Depth16Unorm | crate::descriptors::Format::Depth32Float
        ) {
            4
        } else {
            bytes
        };
        let mut total = 0u64;
        for mip in 0..self.shape.mip_levels {
            total = total
                .checked_next_multiple_of(align)
                .ok_or_else(|| GpuError::Contract("Copy footprint overflow".into()))?;
            let extent = self.shape.extent(self.image.size, mip);
            let size = extent
                .into_iter()
                .chain([self.shape.layers])
                .try_fold(bytes, |n, v| n.checked_mul(u64::from(v)))
                .ok_or_else(|| GpuError::Contract("Copy footprint overflow".into()))?;
            total = total
                .checked_add(size)
                .ok_or_else(|| GpuError::Contract("Copy footprint overflow".into()))?;
        }
        Ok(total)
    }
    pub(crate) fn vk_usage(&self) -> ash::vk::ImageUsageFlags {
        let usage = self.image.vk_usage();
        if self.shape.transient {
            usage | ash::vk::ImageUsageFlags::TRANSIENT_ATTACHMENT
        } else {
            usage
        }
    }
}
/// A rectangular range of one mip, in texels. Layers remain separate from depth.
#[derive(Debug, Clone, Copy)]
pub struct ImageRegion {
    /// Mip level.
    pub mip: u32,
    /// First texel in the mip.
    pub origin: [u32; 3],
    /// Nonzero texel extent.
    pub extent: [u32; 3],
    /// First array layer.
    pub base_layer: u32,
    /// Number of array layers.
    pub layer_count: u32,
}
impl ImageShape {
    pub(crate) fn validate(self, size: [u32; 2]) -> Result<()> {
        if size.contains(&0)
            || self.depth == 0
            || self.layers == 0
            || self.mip_levels == 0
            || self.mip_levels > size[0].max(size[1]).max(self.depth).ilog2() + 1
            || (self.dimension == TextureDimension::D2 && self.depth != 1)
            || (self.dimension == TextureDimension::D3
                && (self.layers != 1 || self.samples != SampleCount::One))
            || (self.samples != SampleCount::One && self.mip_levels != 1)
        {
            return Err(GpuError::Contract("Invalid texture shape".into()));
        }
        Ok(())
    }
    /// Extent of a resident mip.
    pub fn extent(self, size: [u32; 2], mip: u32) -> [u32; 3] {
        [size[0], size[1], self.depth].map(|n| n.checked_shr(mip).unwrap_or(0).max(1))
    }
    pub(crate) fn region(
        self,
        size: [u32; 2],
        region: ImageRegion,
        block: u32,
        bytes: u64,
    ) -> Result<(u64, u64)> {
        let bounds = self.extent(size, region.mip);
        if self.samples != SampleCount::One
            || region.mip >= self.mip_levels
            || region.extent.contains(&0)
            || region.layer_count == 0
            || region
                .base_layer
                .checked_add(region.layer_count)
                .is_none_or(|end| end > self.layers)
            || (0..3).any(|i| {
                region.origin[i]
                    .checked_add(region.extent[i])
                    .is_none_or(|end| end > bounds[i])
            })
            || (0..2).any(|i| {
                !region.origin[i].is_multiple_of(block)
                    || (!region.extent[i].is_multiple_of(block)
                        && region.origin[i] + region.extent[i] != bounds[i])
            })
        {
            return Err(GpuError::Contract("Invalid texture copy region".into()));
        }
        let row = u64::from(region.extent[0].div_ceil(block)) * bytes;
        let plane = row * u64::from(region.extent[1].div_ceil(block));
        let total = plane
            .checked_mul(u64::from(region.extent[2]))
            .and_then(|n| n.checked_mul(u64::from(region.layer_count)))
            .ok_or_else(|| GpuError::Contract("Texture copy footprint overflow".into()))?;
        Ok((row, total))
    }
}

/// Shader-visible interpretation of selected mips and layers, without copying data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewDimension {
    /// One two-dimensional array layer.
    D2,
    /// One or more two-dimensional array layers.
    D2Array,
    /// A volume, with depth selected by the shader.
    D3,
    /// Whole cubes, six consecutive layers each in the order +X, -X, +Y, -Y, +Z, -Z,
    /// sampled by direction and cube index. Sampled views only, over a square
    /// two-dimensional image, where `DeviceInfo::cube_arrays` is set. Write the faces
    /// through `D2` or `D2Array` storage views of the same image.
    CubeArray,
}
/// Immutable view; heap slots retire with the parent image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageViewDesc {
    /// Shader texture dimension.
    pub dimension: ViewDimension,
    /// First resident mip.
    pub base_mip: u32,
    /// Number of mips exposed, one for a storage view.
    pub mip_count: u32,
    /// First array layer (zero for volumes).
    pub base_layer: u32,
    /// Number of array layers (one for volumes).
    pub layer_count: u32,
}
impl ImageViewDesc {
    /// Checks the view against its image's shape and base extent.
    pub(crate) fn validate(self, shape: ImageShape, size: [u32; 2], storage: bool) -> Result<()> {
        let volume = shape.dimension == TextureDimension::D3;
        let cube = self.dimension == ViewDimension::CubeArray;
        if shape.samples != SampleCount::One
            || self.mip_count == 0
            || self.layer_count == 0
            || self
                .base_mip
                .checked_add(self.mip_count)
                .is_none_or(|end| end > shape.mip_levels)
            || self
                .base_layer
                .checked_add(self.layer_count)
                .is_none_or(|end| end > shape.layers)
            || volume != (self.dimension == ViewDimension::D3)
            || (self.dimension == ViewDimension::D2 && self.layer_count != 1)
            || (cube && (storage || size[0] != size[1] || !self.layer_count.is_multiple_of(6)))
            || (storage && (self.mip_count != 1 || shape.samples != SampleCount::One))
        {
            return Err(GpuError::Contract(
                "Invalid texture view range or dimension".into(),
            ));
        }
        Ok(())
    }
}
