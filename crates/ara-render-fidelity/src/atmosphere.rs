//! The physical sky atmosphere and local participating-medium controls, in metres.
use anyhow::{Result, ensure};
use ara_frame::look::atmosphere::{Atmosphere as Model, AtmosphereLight, LightAtmosphere};

/// The shared LUT sky atmosphere (`ara_frame::look::atmosphere`, the same model and
/// shader as the raster look) plus exponential height fog.
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
    /// The planet and its atmosphere (Unreal's SkyAtmosphere in SI units).
    pub model: Model,
    /// How the sun meets the atmosphere; `None` keeps it out of the sky.
    pub sun: Option<LightAtmosphere>,
    /// The sun is the second atmosphere light (index 1, a moon in the sun slot).
    pub sun_is_second: bool,
    /// The other atmosphere light: sky, aerial perspective, sky light and disc only.
    pub other_light: Option<AtmosphereLight>,
    /// World altitude where the fog has its full `fog_extinction`, in metres.
    pub fog_base: f32,
    /// Linear fog scattering colour, from zero to one per channel.
    pub fog_tint: [f32; 3],
    /// Henyey-Greenstein anisotropy of the fog, from zero to 0.9.
    pub fog_anisotropy: f32,
    /// Volumetric fog: shadowed sun shafts and local lights in the fog (the raster
    /// near volume follows it); false is plain height fog.
    pub fog_volumetric: bool,
    /// View depth in metres the raster near fog volume covers (1 to 1000).
    pub fog_volumetric_distance: f32,
    /// Light the fog emits: linear radiance per metre at `fog_extinction`.
    pub fog_emissive: [f32; 3],
    /// Scale of the fog's extinction (not its scattering) on volumetric fog, 0.1 to 10.
    pub fog_extinction_scale: f32,
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
            model: Model::EARTH,
            sun: Some(LightAtmosphere::default()),
            sun_is_second: false,
            other_light: None,
            fog_base: 0.,
            fog_tint: [1.; 3],
            fog_anisotropy: 0.3,
            fog_volumetric: true,
            fog_volumetric_distance: 60.,
            fog_emissive: [0.; 3],
            fog_extinction_scale: 1.,
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
                && self.fog_base.is_finite()
                && self.fog_tint.iter().all(|c| (0.0..=1.0).contains(c))
                && (0.0..=0.9).contains(&self.fog_anisotropy)
                && (1.0..=1000.0).contains(&self.fog_volumetric_distance)
                && self.fog_emissive.iter().all(|c| c.is_finite() && *c >= 0.)
                && (0.1..=10.0).contains(&self.fog_extinction_scale),
            "Invalid atmosphere"
        );
        self.model.validate()
    }
}

#[cfg(test)]
mod tests {
    /// Both renderers cook the same atmosphere: the copy in this shader root is
    /// byte-identical to ara-frame's (`shaders/atmosphere-model.slangh`).
    #[test]
    fn the_atmosphere_shader_is_the_raster_copy() {
        assert_eq!(
            include_str!("../shaders/atmosphere-model.slangh"),
            ara_frame::look::atmosphere::shader_source(),
            "copy crates/ara-frame/shaders/atmosphere-model.slangh into crates/ara-render-fidelity/shaders/"
        );
    }
}
