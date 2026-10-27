//! Colour lookup tables of post-process volumes, decoded once per asset generation
//! outside frame extraction (`Library::luts`, like materials); extraction shares them
//! with renderers through `SceneLook::luts`.
use crate::assets::{AssetSource, asset_id};
use crate::{Result, RuntimeError};
use ara_asset::ArtifactKind;
use ara_components::post::{ColorLut, PostModule, PostProcessVolume};
use bevy_ecs::world::World;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Decoded lookup tables by asset ID. A failed reload keeps the last good table.
#[derive(Default, Clone)]
pub struct Luts {
    entries: BTreeMap<String, Arc<ColorLut>>,
    wanted: BTreeSet<String>,
}

impl Luts {
    /// The latest successfully decoded table.
    pub fn get(&self, asset: &str) -> Option<&Arc<ColorLut>> {
        self.entries.get(asset)
    }

    /// Decodes `asset`'s cooked texture (imported with the `lut` texture usage:
    /// uncompressed half floats, one level) and reports whether the table changed.
    pub fn load(&mut self, source: &dyn AssetSource, asset: &str) -> Result<bool> {
        self.wanted.insert(asset.to_owned());
        let lut = decode(source, asset)?;
        if self.entries.get(asset).is_some_and(|old| **old == lut) {
            return Ok(false);
        }
        self.entries.insert(asset.to_owned(), Arc::new(lut));
        Ok(true)
    }

    /// Every requested asset, including failures eligible for a later reload.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.wanted.iter().map(String::as_str)
    }
}

/// The lookup-table assets the post-process volumes in `world` reference.
pub fn references(world: &mut World) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    let mut query = world.query::<&PostProcessVolume>();
    for volume in query.iter(world) {
        for entry in &volume.overrides {
            if entry.module == PostModule::ColorGrading
                && let Some(lut) = &entry.lut
                && !lut.0.is_empty()
            {
                ids.insert(asset_id(lut)?.to_owned());
            }
        }
    }
    Ok(ids)
}

/// The lookup-table assets one scene record's `PostProcessVolume` references (hosts
/// that build frames from documents, like the editor, load them with `Luts::load`).
pub fn record_references(record: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let entries = record
        .get("PostProcessVolume")
        .and_then(|volume| volume.get("overrides"))
        .and_then(serde_json::Value::as_array);
    entries
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry.get("module").and_then(serde_json::Value::as_str) == Some("colorGrading")
        })
        .filter_map(|entry| entry.get("lut")?.as_str())
        .filter_map(|reference| {
            ara_schema::grammar::parse_ref(reference)
                .ok()?
                .id
                .map(str::to_owned)
        })
        .collect()
}

/// A strip of `size` slices, `size`² × `size` pixels, quantized to RGBA8.
fn decode(source: &dyn AssetSource, asset: &str) -> Result<ColorLut> {
    let invalid = |why: &str| RuntimeError(format!("{asset}: {why}"));
    let (_, cached) = source
        .load(asset)?
        .ok_or_else(|| invalid("the lookup table is not imported"))?;
    let artifact = cached
        .output
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Texture && a.name == "texture.ktx2")
        .ok_or_else(|| invalid("the lookup table is not a texture import"))?;
    let texture = ara_asset_format::Texture::read(&artifact.bytes)
        .map_err(|error| invalid(&error.to_string()))?;
    if texture.format() != ara_asset_format::TextureFormat::Rgba16Float {
        return Err(invalid(
            "import the lookup table with the `lut` texture usage (uncompressed)",
        ));
    }
    let [width, height] = texture.dimensions();
    if !(2..=32).contains(&height) || width != height * height {
        return Err(invalid(
            "a lookup table strip is size² × size pixels (256×16 or 1024×32)",
        ));
    }
    let level = texture
        .levels()
        .next()
        .ok_or_else(|| invalid("the texture has no levels"))?;
    let texels = level
        .as_chunks::<8>()
        .0
        .iter()
        .map(|texel| {
            let channel = |i: usize| {
                let half = u16::from_le_bytes([texel[i * 2], texel[i * 2 + 1]]);
                (half_to_f32(half).clamp(0., 1.) * 255.).round() as u32
            };
            channel(0) | channel(1) << 8 | channel(2) << 16 | channel(3) << 24
        })
        .collect();
    Ok(ColorLut {
        size: height,
        texels,
    })
}

/// IEEE 754 binary16 to f32 (the cooked half-float texels).
fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits >> 15 == 1 { -1. } else { 1. };
    let (exponent, mantissa) = (i32::from((bits >> 10) & 31), f32::from(bits & 1023));
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1. + mantissa / 1024.) * 2f32.powi(e - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::half_to_f32;

    #[test]
    fn records_name_their_volumes_lookup_tables() {
        let record = serde_json::json!({"PostProcessVolume": {"overrides": [
            {"module": "colorGrading", "lut": "@luts/warm.png#as_warm"},
            {"module": "vignette", "lut": "@luts/stale.png#as_stale"}
        ]}});
        let record = record.as_object().expect("a record object");
        assert_eq!(super::record_references(record), ["as_warm"]);
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(half_to_f32(0x3c00), 1.);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0), 0.);
        assert_eq!(half_to_f32(0xbc00), -1.);
    }
}
