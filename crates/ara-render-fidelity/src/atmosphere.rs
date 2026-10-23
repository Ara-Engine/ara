//! Earth atmosphere and local participating-medium controls, in metres.
use anyhow::{Result, ensure};

/// Hillaire-style low-dimensional LUT atmosphere. Disabled views retain their
/// extracted environment; enabled views use the sky LUT for transport and display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    /// World-space altitude of sea level, in metres.
    pub sea_level: f32,
    /// Sea-level local fog extinction, in inverse metres.
    pub fog_extinction: f32,
    /// Exponential fog density scale height, in metres.
    pub fog_height: f32,
    /// Fog single-scattering albedo, from zero to one.
    pub fog_albedo: f32,
    /// Lit planar cloud coverage, from zero to one; zero disables clouds.
    pub cloud_coverage: f32,
    /// Altitude of the cloud layer above sea level, in metres.
    pub cloud_height: f32,
}
impl Default for Atmosphere {
    fn default() -> Self {
        Self {
            sea_level: 0.,
            fog_extinction: 0.0008,
            fog_height: 90.,
            fog_albedo: 0.9,
            cloud_coverage: 0.42,
            cloud_height: 2200.,
        }
    }
}
impl Atmosphere {
    pub(crate) fn validate(self) -> Result<()> {
        ensure!(
            self.sea_level.is_finite()
                && self.fog_extinction.is_finite()
                && (0.0..=0.1).contains(&self.fog_extinction)
                && self.fog_height.is_finite()
                && self.fog_height >= 1.
                && self.cloud_height.is_finite()
                && self.cloud_height >= 500.
                && (0.0..=1.0).contains(&self.fog_albedo)
                && (0.0..=1.0).contains(&self.cloud_coverage),
            "Invalid atmosphere"
        );
        Ok(())
    }
}
