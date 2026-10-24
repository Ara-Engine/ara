//! Compressed mip payloads for native texture residency. Preparation never expands
//! BC blocks into pixels; staging copies only the levels selected by the resident set.
use anyhow::{Result, ensure};
use ara_asset_format::{Texture, TextureFormat};
use std::{ops::Range, sync::Arc};

/// A validated cooked BC texture retaining its original compressed allocation.
/// Prepare it on the asset worker and share it across material instances.
pub struct NativeSource {
    bytes: Arc<[u8]>,
    size: [u32; 2],
    format: TextureFormat,
    levels: Vec<Range<usize>>,
}

/// One borrowed compressed mip, ready for a native image upload.
pub struct NativeMip<'a> {
    /// Texel extent, including dimensions smaller than a compression block.
    pub size: [u32; 2],
    /// Complete GPU-native BC blocks, without container headers.
    pub bytes: &'a [u8],
}

impl NativeSource {
    /// Validates cooked BC4/BC5/BC7 mips without decoding or copying them.
    pub fn from_cooked(bytes: Arc<[u8]>) -> Result<Self> {
        let texture = Texture::read(&bytes)?;
        let size = texture.dimensions();
        let format = texture.format();
        ensure!(
            matches!(
                format,
                TextureFormat::Bc4
                    | TextureFormat::Bc5
                    | TextureFormat::Bc7
                    | TextureFormat::Bc7Srgb
            ),
            "native material residency requires BC4, BC5 or BC7"
        );
        let base = bytes.as_ptr() as usize;
        let levels: Vec<_> = texture
            .levels()
            .map(|level| {
                // These slices borrow the validated container allocation. Integer
                // offsets keep the source movable without self-referential borrows.
                let start = level.as_ptr() as usize - base;
                start..start + level.len()
            })
            .collect();
        Ok(Self {
            bytes,
            size,
            format,
            levels,
        })
    }

    /// Finest-level width and height in texels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Native encoding and transfer function; BC5 stores normal XY only.
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// Number of encoded levels. Authored textures with mipmaps disabled retain
    /// their single level; the sampler clamps to the last resident mip.
    pub fn mip_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Borrows the original encoded blocks for a level, without pixel conversion.
    pub fn mip(&self, level: u32) -> Option<NativeMip<'_>> {
        let range = self.levels.get(level as usize)?;
        Some(NativeMip {
            size: self.size.map(|v| (v >> level).max(1)),
            bytes: &self.bytes[range.start..range.end],
        })
    }

    /// GPU allocation payload for a resident mip tail, excluding KTX2 headers.
    pub fn resident_bytes(&self, first_mip: u32) -> Option<u64> {
        let levels = self.levels.get(first_mip as usize..)?;
        if levels.is_empty() {
            return None;
        }
        Some(levels.iter().map(|range| range.len() as u64).sum())
    }

    /// Copies the selected tail into tightly packed, finest-to-coarsest staging
    /// bytes. BC payload sizes keep every native copy offset block-aligned.
    pub fn stage(&self, first_mip: u32) -> Result<Vec<u8>> {
        let size = self
            .resident_bytes(first_mip)
            .ok_or_else(|| anyhow::anyhow!("resident mip is out of range"))?;
        let mut out = Vec::with_capacity(usize::try_from(size)?);
        for range in &self.levels[first_mip as usize..] {
            out.extend_from_slice(&self.bytes[range.start..range.end]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_asset_format::write_ktx2;

    #[test]
    fn native_mip_tails_preserve_bc_blocks_and_small_level_sizes() {
        for format in [
            TextureFormat::Bc7Srgb,
            TextureFormat::Bc5,
            TextureFormat::Bc4,
        ] {
            let block = format.block_bytes();
            let levels = vec![
                vec![11; 4 * block],
                vec![22; block],
                vec![33; block],
                vec![44; block],
            ];
            let bytes: Arc<[u8]> = write_ktx2(8, 8, format, &levels).unwrap().into();
            let source = NativeSource::from_cooked(Arc::clone(&bytes)).unwrap();
            assert!(
                Arc::ptr_eq(&source.bytes, &bytes),
                "keeps the original allocation"
            );
            assert_eq!(source.mip_count(), 4);
            assert_eq!(source.format(), format);
            assert_eq!(source.resident_bytes(0), Some((7 * block) as u64));
            assert_eq!(source.resident_bytes(2), Some((2 * block) as u64));
            for (i, expected) in levels.iter().enumerate() {
                let mip = source.mip(i as u32).unwrap();
                assert_eq!(mip.size, [(8 >> i).max(1); 2]);
                assert_eq!(mip.bytes, expected);
            }
            assert_eq!(
                source.stage(2).unwrap(),
                [levels[2].as_slice(), levels[3].as_slice()].concat()
            );
            assert!(source.mip(4).is_none());
            assert!(source.stage(4).is_err());
        }
    }

    #[test]
    fn mipless_assets_stay_native_and_truncated_payloads_are_rejected() {
        let bytes = write_ktx2(8, 8, TextureFormat::Bc5, &[vec![0; 64]]).unwrap();
        let truncated = bytes[..bytes.len() - 1].to_vec();
        let source = NativeSource::from_cooked(bytes.into()).unwrap();
        assert_eq!(source.mip_count(), 1);
        assert_eq!(source.resident_bytes(0), Some(64));
        assert!(NativeSource::from_cooked(truncated.into()).is_err());
    }
}
