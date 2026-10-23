//! Probe lattice and portable baked irradiance/visibility storage.
use anyhow::{Result, ensure};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// One probe uses 6x6 directional irradiance, 8x8 visibility and two metadata vectors.
pub const PROBE_VECTORS: usize = 102;
/// A bounded automatic world-space lattice. Cell size is in metres.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ProbeVolume {
    /// First probe centre.
    pub origin: Vec3,
    /// Independent XYZ probe separation.
    pub spacing: Vec3,
    /// XYZ lattice dimensions; each axis has at least two probes.
    pub counts: [u32; 3],
}
impl ProbeVolume {
    /// Enclose geometry with half a padding cell and a bounded lattice.
    /// Probe centres avoid the bounding planes, where zero-distance hits vanish.
    pub fn automatic(min: Vec3, max: Vec3) -> Result<Self> {
        ensure!(
            min.is_finite() && max.is_finite() && min.cmple(max).all(),
            "Invalid probe bounds"
        );
        let extent = (max - min).max(Vec3::ONE);
        ensure!(extent.is_finite(), "Probe bounds overflow");
        // Keep small diagnostic volumes stable while bounding spacing in real scenes.
        let minimum = [10, 6, 10];
        let mut counts = std::array::from_fn(|i| {
            ((extent[i] / 1.5).ceil().min(62.) as u32 + 2).clamp(minimum[i], 64)
        });
        while counts.iter().map(|n| *n as usize).product::<usize>() > 32768 {
            let axis = (0..3).max_by_key(|i| counts[*i]).expect("Three probe axes");
            counts[axis] -= 1;
        }
        let spacing = extent
            / Vec3::new(
                (counts[0] - 2) as f32,
                (counts[1] - 2) as f32,
                (counts[2] - 2) as f32,
            );
        let volume = Self {
            origin: min - spacing * 0.5,
            spacing,
            counts,
        };
        volume.validate()?;
        Ok(volume)
    }
    /// Validate imported baked data before GPU upload.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.origin.is_finite()
                && self.spacing.is_finite()
                && self.spacing.cmpgt(Vec3::ZERO).all(),
            "Invalid probe positions"
        );
        ensure!(
            self.counts.iter().all(|n| (2..=64).contains(n)) && self.len() <= 32768,
            "Invalid probe dimensions"
        );
        Ok(())
    }
    /// Total probe count, bounded by `validate`.
    pub fn len(&self) -> usize {
        self.counts
            .iter()
            .fold(1usize, |n, v| n.saturating_mul(*v as usize))
    }
    /// A valid volume never contains zero probes.
    pub fn is_empty(&self) -> bool {
        self.counts.contains(&0)
    }
    /// Allocate explicitly uninitialized probes; classification keeps them out of shading.
    pub fn empty_data(&self) -> Vec<[f32; 4]> {
        vec![[0.; 4]; self.len() * PROBE_VECTORS]
    }
}
/// Cooked fallback shared by Raster and Mobile. Irradiance is linear, visibility
/// stores distance moments in metres, and metadata stores relocation/classification.
#[derive(Serialize, Deserialize)]
pub struct BakedProbes {
    /// Packed directional-probe ABI version; currently one.
    pub version: u32,
    /// Original lattice coordinate system.
    pub volume: ProbeVolume,
    /// SHA-256 of the contributing scene; mismatches are rejected.
    pub scene_hash: String,
    /// Packed directional irradiance, visibility and metadata records.
    pub data: Vec<[f32; 4]>,
}
impl BakedProbes {
    /// Check shape, finite radiance and scene provenance.
    pub fn validate(&self, hash: &str) -> Result<()> {
        ensure!(self.version == 1, "Unsupported baked probe version");
        self.volume.validate()?;
        ensure!(
            self.scene_hash == hash,
            "Baked probes belong to a different scene"
        );
        ensure!(
            self.data.len() == self.volume.len() * PROBE_VECTORS
                && self.data.iter().flatten().all(|v| v.is_finite()),
            "Malformed baked probes"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_bounds_budget_and_bake_provenance() {
        let v = ProbeVolume::automatic(Vec3::ZERO, Vec3::splat(10.)).unwrap();
        v.validate().unwrap();
        assert!(v.origin.cmplt(Vec3::ZERO).all());
        let large = ProbeVolume::automatic(Vec3::ZERO, Vec3::splat(1e20)).unwrap();
        assert!(large.len() <= 32768);
        assert!(ProbeVolume::automatic(Vec3::splat(-f32::MAX), Vec3::splat(f32::MAX)).is_err());
        let bake = BakedProbes {
            version: 1,
            volume: v,
            scene_hash: "scene".into(),
            data: v.empty_data(),
        };
        bake.validate("scene").unwrap();
        assert!(bake.validate("stale").is_err());
    }
}
