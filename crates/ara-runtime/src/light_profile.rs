//! Photometric light profiles (`Light.profile`: imported IES files, `ara.ies`), decoded
//! once per asset generation outside frame extraction (`Library::light_profiles`, like
//! lookup tables); extraction shares them with renderers through `LightView::profile`.
use crate::assets::{AssetSource, asset_id};
use crate::components::Light;
use crate::{Result, RuntimeError};
use ara_asset::ArtifactKind;
use bevy_ecs::world::World;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A light's intensity by direction, normalised to its peak.
#[derive(Debug, Clone, PartialEq)]
pub struct LightProfile {
    /// Vertical samples from nadir (the light's -Z axis) to zenith, inclusive.
    pub vertical: u32,
    /// Azimuth samples over 360 degrees from the light's +X axis (one: symmetric).
    pub horizontal: u32,
    /// Measured peak luminous intensity in candela.
    pub peak: f32,
    /// Measured total luminous flux in lumens.
    pub lumens: f32,
    /// Intensity over the peak, azimuth-major (`vertical` per azimuth sample).
    pub values: Vec<f32>,
}

impl LightProfile {
    /// The profile at a direction from the light, in its frame: `cos_nadir` the cosine
    /// from its -Z axis, `azimuth` radians from +X towards +Y (bilinear, as shaders).
    pub fn sample(&self, cos_nadir: f32, azimuth: f32) -> f32 {
        let v = cos_nadir.clamp(-1., 1.).acos() / std::f32::consts::PI * (self.vertical - 1) as f32;
        let h = azimuth.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
            * self.horizontal as f32;
        let (v0, h0) = (v.floor() as u32, h.floor() as u32);
        let (tv, th) = (v - v0 as f32, h - h0 as f32);
        let at = |h: u32, v: u32| {
            self.values[((h % self.horizontal) * self.vertical + v.min(self.vertical - 1)) as usize]
        };
        let row = |h| at(h, v0) * (1. - tv) + at(h, v0 + 1) * tv;
        row(h0) * (1. - th) + row(h0 + 1) * th
    }
}

/// Decoded profiles by asset ID. A failed reload keeps the last good profile.
#[derive(Default, Clone)]
pub struct LightProfiles {
    entries: BTreeMap<String, Arc<LightProfile>>,
    wanted: BTreeSet<String>,
}

impl LightProfiles {
    /// The latest successfully decoded profile.
    pub fn get(&self, asset: &str) -> Option<&Arc<LightProfile>> {
        self.entries.get(asset)
    }

    /// Decodes `asset`'s `profile.bin` and reports whether the profile changed.
    pub fn load(&mut self, source: &dyn AssetSource, asset: &str) -> Result<bool> {
        self.wanted.insert(asset.to_owned());
        let profile = decode(source, asset)?;
        if self.entries.get(asset).is_some_and(|old| **old == profile) {
            return Ok(false);
        }
        self.entries.insert(asset.to_owned(), Arc::new(profile));
        Ok(true)
    }

    /// Every requested asset, including failures eligible for a later reload.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.wanted.iter().map(String::as_str)
    }
}

/// The profile assets the lights in `world` reference.
pub fn references(world: &mut World) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    let mut query = world.query::<&Light>();
    for light in query.iter(world) {
        if let Some(profile) = light.profile.as_ref().filter(|p| !p.0.is_empty()) {
            ids.insert(asset_id(profile)?.to_owned());
        }
    }
    Ok(ids)
}

/// The profile asset one scene record's `Light` references (hosts that build frames
/// from documents, like the editor, load it with `LightProfiles::load`).
pub fn record_references(record: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let reference = record.get("Light")?.get("profile")?.as_str()?;
    ara_schema::grammar::parse_ref(reference)
        .ok()?
        .id
        .map(str::to_owned)
}

fn decode(source: &dyn AssetSource, asset: &str) -> Result<LightProfile> {
    let invalid = |why: &str| RuntimeError(format!("{asset}: {why}"));
    let (_, cached) = source
        .load(asset)?
        .ok_or_else(|| invalid("the light profile is not imported"))?;
    let bytes = &cached
        .output
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Custom && a.name == "profile.bin")
        .ok_or_else(|| invalid("not an IES light profile import (`ara.ies`)"))?
        .bytes;
    let word = |i: usize| -> Option<[u8; 4]> { bytes.get(8 + i * 4..12 + i * 4)?.try_into().ok() };
    if bytes.get(..8) != Some(b"ARAIES01") {
        return Err(invalid("the light profile has an unknown format"));
    }
    let header = |i| word(i).ok_or_else(|| invalid("the light profile is truncated"));
    let (vertical, horizontal) = (
        u32::from_le_bytes(header(0)?),
        u32::from_le_bytes(header(1)?),
    );
    let (peak, lumens) = (
        f32::from_le_bytes(header(2)?),
        f32::from_le_bytes(header(3)?),
    );
    let count = vertical as usize * horizontal as usize;
    if !(2..=256).contains(&vertical)
        || !(1..=256).contains(&horizontal)
        || bytes.len() != 24 + count * 4
    {
        return Err(invalid("the light profile's grid is malformed"));
    }
    let values = (0..count)
        .map(|i| header(4 + i).map(f32::from_le_bytes))
        .collect::<Result<Vec<_>>>()?;
    if !(peak.is_finite() && peak > 0.) || values.iter().any(|v| !v.is_finite()) {
        return Err(invalid("the light profile has invalid intensities"));
    }
    Ok(LightProfile {
        vertical,
        horizontal,
        peak,
        lumens,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_name_their_profile_and_samples_interpolate() {
        let record =
            serde_json::json!({"Light": {"kind": "spot", "profile": "@lights/down.ies#as_down"}});
        let record = record.as_object().expect("a record object");
        assert_eq!(record_references(record).as_deref(), Some("as_down"));
        let profile = LightProfile {
            vertical: 3,
            horizontal: 1,
            peak: 100.,
            lumens: 1.,
            values: vec![1., 0.5, 0.],
        };
        assert_eq!(profile.sample(1., 0.), 1., "nadir");
        assert!((profile.sample(0., 2.).abs() - 0.5).abs() < 1e-6, "horizon");
        assert!((profile.sample((std::f32::consts::FRAC_PI_4).cos(), 0.) - 0.75).abs() < 1e-5);
    }
}
