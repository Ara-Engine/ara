//! Bounded bulk transport. Texels and encoded blocks are raw little-endian bytes.
use ara_asset::{CookPreset, Error, Result};
use ara_asset_format::TextureFormat;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

pub(super) const MAX_BLOCKS: usize = super::super::dispatch::MAX_BLOCKS;
const MAX_JSON: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Descriptor {
    pub endpoint: ara_asset::EncoderEndpoint,
    pub identity: String,
    pub pid: u32,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Reply {
    pub error: Option<String>,
    pub bytes: usize,
    pub metrics: BTreeMap<String, f64>,
}

pub(super) fn write_json(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let value = serde_json::to_value(value)?;
    let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&value)).into_bytes();
    if bytes.len() > MAX_JSON {
        return Err(Error::Invalid(
            "codec service metadata exceeds limit".into(),
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub(super) fn read_json<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_JSON {
        return Err(Error::Invalid(
            "codec service metadata exceeds limit".into(),
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) fn format(id: u32) -> Result<TextureFormat> {
    match id {
        139 => Ok(TextureFormat::Bc4),
        141 => Ok(TextureFormat::Bc5),
        143 => Ok(TextureFormat::Bc6h),
        145 => Ok(TextureFormat::Bc7),
        146 => Ok(TextureFormat::Bc7Srgb),
        _ => Err(Error::Invalid("unsupported codec service format".into())),
    }
}
pub(super) fn preset(id: u8) -> Result<CookPreset> {
    match id {
        0 => Ok(CookPreset::Fast),
        1 => Ok(CookPreset::Balanced),
        2 => Ok(CookPreset::HighQuality),
        _ => Err(Error::Invalid("unsupported codec service preset".into())),
    }
}
pub(super) fn preset_id(preset: CookPreset) -> u8 {
    match preset {
        CookPreset::Fast => 0,
        CookPreset::Balanced => 1,
        CookPreset::HighQuality => 2,
    }
}
pub(super) fn words(format: TextureFormat) -> usize {
    if format == TextureFormat::Bc6h {
        32
    } else {
        16
    }
}
pub(super) fn validate_count(format: TextureFormat, count: usize) -> Result<usize> {
    let words = words(format);
    if count > MAX_BLOCKS * words || !count.is_multiple_of(words) {
        return Err(Error::Invalid(
            "codec service block count exceeds limit or is truncated".into(),
        ));
    }
    Ok(count / words * format.block_bytes())
}

pub(super) fn write_words(writer: &mut impl Write, words: &[u32]) -> Result<()> {
    #[cfg(target_endian = "little")]
    writer.write_all(bytemuck::cast_slice(words))?;
    #[cfg(target_endian = "big")]
    for word in words {
        writer.write_all(&word.to_le_bytes())?;
    }
    Ok(())
}
pub(super) fn read_words(reader: &mut impl Read, count: usize) -> Result<Vec<u32>> {
    let mut words = vec![0u32; count];
    reader.read_exact(bytemuck::cast_slice_mut(&mut words))?;
    #[cfg(target_endian = "big")]
    for word in &mut words {
        *word = word.swap_bytes();
    }
    Ok(words)
}
