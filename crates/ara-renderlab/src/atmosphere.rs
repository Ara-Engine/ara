//! Authored `SkyAtmosphere` and `HeightFog` as Renderlab's atmosphere medium and fog.
//! Ara's sky is Unreal's SkyAtmosphere value for value (kilometres, coefficients per
//! kilometre), and so is Renderlab's medium (Hillaire 2020), so the physical parameters
//! carry over directly. Artistic controls with no Renderlab counterpart are reported
//! when they differ from their defaults (`ARA_RENDERLAB_UNSUPPORTED`).
use crate::gaps::Gaps;
use ara_components::render::{HeightFog as AraFog, SkyAtmosphere};
use renderlab::engine::{AtmosphereMedium, HeightFog, VolumetricFog};

fn times(scale: f32, colour: [f32; 3]) -> [f32; 3] {
    colour.map(|c| (c * scale).max(0.))
}

/// Renderlab's medium for an authored sky atmosphere.
pub(crate) fn medium(sky: &SkyAtmosphere, gaps: &mut Gaps) -> AtmosphereMedium {
    let defaults = SkyAtmosphere::default();
    let artistic = sky.multi_scattering != defaults.multi_scattering
        || sky.sky_luminance_factor != defaults.sky_luminance_factor
        || sky.sky_and_aerial_perspective_luminance_factor
            != defaults.sky_and_aerial_perspective_luminance_factor
        || sky.aerial_perspective_view_distance_scale
            != defaults.aerial_perspective_view_distance_scale
        || sky.height_fog_contribution != defaults.height_fog_contribution
        || sky.transmittance_min_light_elevation_angle
            != defaults.transmittance_min_light_elevation_angle
        || sky.aerial_perspective_start_depth != defaults.aerial_perspective_start_depth;
    if artistic {
        gaps.unsupported(
            "sky atmosphere luminance factors, multiple-scattering strength and aerial-perspective controls",
            "the sky is drawn physically from its medium",
        );
    }
    if sky.cloud_coverage > 0. {
        gaps.unsupported("the sky atmosphere's cloud layer", "the sky is clear");
    }
    AtmosphereMedium {
        ground_radius_km: sky.bottom_radius,
        top_radius_km: sky.bottom_radius + sky.atmosphere_height.max(0.1),
        ground_height: sky.sea_level,
        rayleigh_scattering: times(sky.rayleigh_scattering_scale, sky.rayleigh_scattering),
        rayleigh_scale_height_km: sky.rayleigh_exponential_distribution.max(1e-3),
        mie_scattering: times(sky.mie_scattering_scale, sky.mie_scattering),
        mie_absorption: times(sky.mie_absorption_scale, sky.mie_absorption),
        mie_scale_height_km: sky.mie_exponential_distribution.max(1e-3),
        mie_g: sky.mie_anisotropy.clamp(0., 0.999),
        // The tent's peak density times its absorption; a zero width removes the layer.
        ozone_absorption: if sky.absorption_width > 0. {
            times(
                sky.absorption_scale * sky.absorption_tip_value,
                sky.absorption,
            )
        } else {
            [0.; 3]
        },
        ozone_peak_km: sky.absorption_tip_altitude,
        ozone_half_width_km: sky.absorption_width.max(1e-3),
        ground_albedo: sky.ground_albedo.map(|c| c.clamp(0., 1.)),
        ..AtmosphereMedium::default()
    }
}

/// Renderlab's height fog, and its volumetric fog when the authored fog is volumetric.
pub(crate) fn fog(fog: &AraFog, gaps: &mut Gaps) -> (HeightFog, Option<VolumetricFog>) {
    let defaults = AraFog::default();
    if fog.volumetric_emissive != defaults.volumetric_emissive
        || fog.volumetric_extinction_scale != defaults.volumetric_extinction_scale
    {
        gaps.unsupported(
            "volumetric fog emission and extinction scale",
            "the fog neither glows nor darkens beyond its density",
        );
    }
    let [r, g, b, _] = fog.albedo.0;
    let height = HeightFog {
        density: fog.density.max(0.),
        // Ara gives the scale height in metres; Renderlab its inverse.
        falloff: 1. / fog.falloff.max(1e-3),
        base_height: fog.base_height,
        albedo: [r, g, b].map(|c| c.clamp(0., 1.)),
        anisotropy: fog.anisotropy.clamp(-0.99, 0.99),
        ..HeightFog::default()
    };
    let volumetric = fog.volumetric.then(|| VolumetricFog {
        distance: fog.volumetric_distance.max(1.),
        ..VolumetricFog::default()
    });
    (height, volumetric)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_sky_is_renderlabs_earth() {
        let mut gaps = Gaps::default();
        let m = medium(&SkyAtmosphere::default(), &mut gaps);
        let earth = AtmosphereMedium::default();
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 2e-4);
        assert!(close(m.rayleigh_scattering, earth.rayleigh_scattering));
        assert!(close(m.mie_scattering, earth.mie_scattering));
        assert!(close(m.ozone_absorption, earth.ozone_absorption));
        assert_eq!((m.ground_radius_km, m.top_radius_km), (6360., 6420.));
        assert!(!gaps.reported(
            "sky atmosphere luminance factors, multiple-scattering strength and aerial-perspective controls"
        ));
    }

    #[test]
    fn fog_scale_height_becomes_a_falloff() {
        let mut gaps = Gaps::default();
        let (h, v) = fog(
            &AraFog {
                falloff: 50.,
                ..AraFog::default()
            },
            &mut gaps,
        );
        assert!((h.falloff - 0.02).abs() < 1e-6);
        assert_eq!(v.is_some(), AraFog::default().volumetric);
    }
}
