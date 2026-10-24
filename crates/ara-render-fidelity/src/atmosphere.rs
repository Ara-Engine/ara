//! Earth atmosphere and local participating-medium controls, in metres.
use anyhow::{Result, ensure};

/// Hillaire-style low-dimensional LUT atmosphere plus exponential height fog.
/// Disabled views retain their extracted environment. With `physical_sky` the sky
/// LUT also replaces the environment map for transport and display; without it the
/// environment map stays the sky and only the height fog (lit by the sun, local
/// lights and the environment's ambient) is added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    /// World-space altitude of sea level, in metres.
    pub sea_level: f32,
    /// Local fog extinction at `fog_base`, in inverse metres.
    pub fog_extinction: f32,
    /// Exponential fog density scale height, in metres.
    pub fog_height: f32,
    /// Fog single-scattering albedo, from zero to one; multiplies `fog_tint`.
    pub fog_albedo: f32,
    /// Lit planar cloud coverage, from zero to one; zero disables clouds.
    pub cloud_coverage: f32,
    /// Altitude of the cloud layer above sea level, in metres.
    pub cloud_height: f32,
    /// Use the physical sky as the environment; false keeps the extracted environment map.
    pub physical_sky: bool,
    /// Aerosol (Mie) density relative to clear Earth air.
    pub aerosols: f32,
    /// World altitude where the fog has its full `fog_extinction`, in metres.
    pub fog_base: f32,
    /// Linear fog scattering colour, from zero to one per channel.
    pub fog_tint: [f32; 3],
    /// Henyey-Greenstein anisotropy of the fog, from zero to 0.9.
    pub fog_anisotropy: f32,
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
            physical_sky: true,
            aerosols: 1.,
            fog_base: 0.,
            fog_tint: [1.; 3],
            fog_anisotropy: 0.3,
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
                && (0.0..=1.0).contains(&self.cloud_coverage)
                && (0.0..=20.0).contains(&self.aerosols)
                && self.fog_base.is_finite()
                && self.fog_tint.iter().all(|c| (0.0..=1.0).contains(c))
                && (0.0..=0.9).contains(&self.fog_anisotropy),
            "Invalid atmosphere"
        );
        Ok(())
    }
}
