use crate::FormatError;

/// GPU-native desktop texture formats. ASTC encoders can extend this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// Linear RGBA BC7.
    Bc7,
    /// sRGB RGB, linear alpha BC7.
    Bc7Srgb,
    /// Unsigned RG normal map (reconstruct positive Z).
    Bc5,
    /// Unsigned single-channel mask.
    Bc4,
    /// Nonnegative HDR RGB in half precision.
    Bc6h,
    /// Signed HDR or HDR alpha, stored as RGBA half floats.
    Rgba16Float,
    /// HDR values outside half-float range, retained as RGBA float32.
    Rgba32Float,
    /// Mobile ASTC 4×4 linear; produced by a plugin encoder.
    Astc4x4,
    /// Mobile ASTC 4×4 sRGB; produced by a plugin encoder.
    Astc4x4Srgb,
    /// Mobile ASTC 6×6 linear; produced by a plugin encoder.
    Astc6x6,
    /// Mobile ASTC 6×6 sRGB; produced by a plugin encoder.
    Astc6x6Srgb,
    /// Mobile ASTC 8×8 linear; produced by a plugin encoder.
    Astc8x8,
    /// Mobile ASTC 8×8 sRGB; produced by a plugin encoder.
    Astc8x8Srgb,
}
impl TextureFormat {
    /// Corresponding Vulkan VkFormat, shared with KTX2.
    pub fn vk_format(self) -> u32 {
        match self {
            Self::Bc7 => 145,
            Self::Bc7Srgb => 146,
            Self::Bc5 => 141,
            Self::Bc4 => 139,
            Self::Bc6h => 143,
            Self::Rgba16Float => 97,
            Self::Rgba32Float => 109,
            Self::Astc4x4 => 157,
            Self::Astc4x4Srgb => 158,
            Self::Astc6x6 => 165,
            Self::Astc6x6Srgb => 166,
            Self::Astc8x8 => 171,
            Self::Astc8x8Srgb => 172,
        }
    }
    /// Bytes per compressed 4×4 block.
    pub fn block_bytes(self) -> usize {
        if matches!(self, Self::Bc4 | Self::Rgba16Float) {
            8
        } else {
            16
        }
    }
    /// Texels covered by one encoded block; raw float formats use one texel.
    pub fn block_extent(self) -> u32 {
        match self {
            Self::Rgba16Float | Self::Rgba32Float => 1,
            Self::Astc6x6 | Self::Astc6x6Srgb => 6,
            Self::Astc8x8 | Self::Astc8x8Srgb => 8,
            _ => 4,
        }
    }
    fn type_size(self) -> u32 {
        match self {
            Self::Rgba16Float => 2,
            Self::Rgba32Float => 4,
            _ => 1,
        }
    }
    fn from_vk(v: u32) -> Result<Self, FormatError> {
        match v {
            145 => Ok(Self::Bc7),
            146 => Ok(Self::Bc7Srgb),
            141 => Ok(Self::Bc5),
            139 => Ok(Self::Bc4),
            143 => Ok(Self::Bc6h),
            97 => Ok(Self::Rgba16Float),
            109 => Ok(Self::Rgba32Float),
            157 => Ok(Self::Astc4x4),
            158 => Ok(Self::Astc4x4Srgb),
            165 => Ok(Self::Astc6x6),
            166 => Ok(Self::Astc6x6Srgb),
            171 => Ok(Self::Astc8x8),
            172 => Ok(Self::Astc8x8Srgb),
            _ => Err(FormatError("unsupported texture format")),
        }
    }
}
/// A validated two-dimensional KTX2 texture with borrowed GPU mip payloads.
pub struct Texture<'a> {
    reader: ktx2::Reader<&'a [u8]>,
    format: TextureFormat,
}
impl<'a> Texture<'a> {
    /// Checks the container and every compressed level's expected byte count.
    pub fn read(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let reader = ktx2::Reader::new(bytes).map_err(|_| FormatError("invalid KTX2"))?;
        let h = reader.header();
        let format =
            TextureFormat::from_vk(h.format.ok_or(FormatError("missing VkFormat"))?.value())?;
        if crate::container::u32_at(bytes, 44)? != 0 {
            return Err(FormatError("unsupported supercompression"));
        }
        let (expected, _) =
            ktx2::dfd::Basic::from_format(h.format.ok_or(FormatError("missing VkFormat"))?)
                .map_err(|_| FormatError("invalid format DFD"))?;
        let basic = reader.basic_dfd().ok_or(FormatError("missing basic DFD"))?;
        if basic.color_model != expected.color_model
            || basic.transfer_function != expected.transfer_function
            || basic.texel_block_dimensions != expected.texel_block_dimensions
            || basic.bytes_planes != expected.bytes_planes
        {
            return Err(FormatError("DFD disagrees with VkFormat"));
        }
        if h.pixel_height == 0
            || h.pixel_depth != 0
            || h.layer_count != 0
            || h.face_count != 1
            || h.supercompression_scheme.is_some()
            || h.type_size != format.type_size()
            || h.level_count == 0
            || h.level_count > 32 - h.pixel_width.max(h.pixel_height).leading_zeros()
        {
            return Err(FormatError("unsupported KTX2 shape"));
        }
        for (i, l) in reader.levels().enumerate() {
            let w = (h.pixel_width >> i).max(1);
            let height = (h.pixel_height >> i).max(1);
            let size = w.div_ceil(format.block_extent()) as u64
                * height.div_ceil(format.block_extent()) as u64
                * format.block_bytes() as u64;
            if l.data.len() as u64 != size || l.uncompressed_byte_length != size {
                return Err(FormatError("invalid compressed mip size"));
            }
            if format == TextureFormat::Rgba16Float
                && l.data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .any(|v| u16::from_le_bytes(*v) & 0x7c00 == 0x7c00)
            {
                return Err(FormatError("nonfinite half texture value"));
            }
            if format == TextureFormat::Rgba32Float
                && crate::container::floats(l.data).any(|v| !v.is_finite())
            {
                return Err(FormatError("nonfinite float texture value"));
            }
        }
        Ok(Self { reader, format })
    }
    /// Texture format and transfer function for GPU creation.
    pub fn format(&self) -> TextureFormat {
        self.format
    }
    /// Base level dimensions.
    pub fn dimensions(&self) -> [u32; 2] {
        let h = self.reader.header();
        [h.pixel_width, h.pixel_height]
    }
    /// Finest-to-coarsest borrowed compressed mip payloads.
    pub fn levels(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.reader.levels().map(|l| l.data)
    }
}

/// Writes KTX2 with Khronos DFDs and streaming order (smallest mip first on disk).
pub fn write_ktx2(
    width: u32,
    height: u32,
    format: TextureFormat,
    levels: &[Vec<u8>],
) -> Result<Vec<u8>, FormatError> {
    if width == 0
        || height == 0
        || levels.is_empty()
        || levels.len() > 32 - width.max(height).leading_zeros() as usize
    {
        return Err(FormatError("invalid texture dimensions"));
    }
    let vk = ktx2::Format::new(format.vk_format()).ok_or(FormatError("invalid VkFormat"))?;
    let (basic, _) = ktx2::dfd::Basic::from_format(vk).map_err(|_| FormatError("invalid DFD"))?;
    let dfd = ktx2::dfd::Block::Basic(basic);
    let dfd_len = 4 + dfd.serialized_length();
    let dfd_offset = 80 + 24 * levels.len();
    let mut bytes = vec![0; dfd_offset + dfd_len];
    let h = ktx2::Header {
        format: Some(vk),
        type_size: format.type_size(),
        pixel_width: width,
        pixel_height: height,
        pixel_depth: 0,
        layer_count: 0,
        face_count: 1,
        level_count: levels.len() as u32,
        supercompression_scheme: None,
        index: ktx2::Index {
            dfd_byte_offset: dfd_offset as u32,
            dfd_byte_length: dfd_len as u32,
            kvd_byte_offset: 0,
            kvd_byte_length: 0,
            sgd_byte_offset: 0,
            sgd_byte_length: 0,
        },
    };
    bytes[..80].copy_from_slice(&h.as_bytes());
    bytes[dfd_offset..dfd_offset + 4].copy_from_slice(&(dfd_len as u32).to_le_bytes());
    dfd.to_bytes(&mut bytes[dfd_offset + 4..]);
    for (i, level) in levels.iter().enumerate().rev() {
        bytes.resize(bytes.len().next_multiple_of(format.block_bytes()), 0);
        let offset = bytes.len() as u64;
        let len = level.len() as u64;
        let p = 80 + 24 * i;
        bytes[p..p + 8].copy_from_slice(&offset.to_le_bytes());
        bytes[p + 8..p + 16].copy_from_slice(&len.to_le_bytes());
        bytes[p + 16..p + 24].copy_from_slice(&len.to_le_bytes());
        bytes.extend(level);
    }
    Texture::read(&bytes)?;
    Ok(bytes)
}
