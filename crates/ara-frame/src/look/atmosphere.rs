//! The physical sky atmosphere both renderers draw (`shaders/atmosphere-model.slangh`):
//! Unreal's SkyAtmosphere parameters in SI units (metres, inverse metres), the two
//! atmosphere lights, and the GPU record ([`record`]) the raster look and
//! ara-render-fidelity upload unchanged.
//!
//! `ara_components::render::SkyAtmosphere` stores the same parameters in Unreal's units
//! (kilometres, inverse kilometres, colour times scale); ara-render-fidelity's
//! `environment` converts them one to one.
use glam::Vec3;

/// The shared shader source (`shaders/atmosphere-model.slangh`). Renderers with their
/// own shader root keep a copy and verify it equals this string.
pub fn shader_source() -> &'static str {
    include_str!("../../shaders/atmosphere-model.slangh")
}

/// Unreal's SkyAtmosphere (an Earth-like default), in SI units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    /// Planet radius to the ground, in metres.
    pub bottom_radius: f32,
    /// Atmosphere thickness above the ground, in metres.
    pub height: f32,
    /// Linear ground albedo lighting the atmosphere from below (multiple scattering
    /// and the planet seen below the horizon).
    pub ground_albedo: [f32; 3],
    /// Multiple-scattering contribution, 0 (single scattering only) to 100; 1 physical.
    pub multi_scattering: f32,
    /// Scales the ray-march sample counts, 0.25 to 8.
    pub trace_sample_count_scale: f32,
    /// Rayleigh scattering coefficients at the ground, in 1/m.
    pub rayleigh_scattering: [f32; 3],
    /// Rayleigh density scale height (density falls by e over it), in metres.
    pub rayleigh_scale_height: f32,
    /// Mie scattering coefficients at the ground, in 1/m.
    pub mie_scattering: [f32; 3],
    /// Mie absorption coefficients at the ground, in 1/m.
    pub mie_absorption: [f32; 3],
    /// Mie phase anisotropy (Henyey-Greenstein g), 0 to 0.999.
    pub mie_anisotropy: f32,
    /// Mie density scale height, in metres.
    pub mie_scale_height: f32,
    /// Tent-layer (ozone) absorption at density 1, in 1/m.
    pub absorption: [f32; 3],
    /// Altitude of the tent's tip, in metres.
    pub absorption_tip_altitude: f32,
    /// Density at the tent's tip (clamped to 0..1 at every altitude).
    pub absorption_tip_value: f32,
    /// Altitude over which the tent falls from its tip to zero on each side, in
    /// metres; 0 removes the layer.
    pub absorption_width: f32,
    /// Linear multiplier on the drawn sky's luminance (and so the sky light).
    pub sky_luminance_factor: [f32; 3],
    /// Linear multiplier on the sky's and the aerial perspective's luminance.
    pub sky_and_aerial_perspective_luminance_factor: [f32; 3],
    /// Stretches distances from the camera to surfaces for the aerial perspective.
    pub aerial_perspective_view_distance_scale: f32,
    /// Distance from the camera where the aerial perspective begins, in metres.
    pub aerial_perspective_start_depth: f32,
    /// Scales the sun and sky light the height fog scatters beyond the volumetric
    /// fog's near volume (Unreal's sky atmosphere contribution to height fog).
    pub height_fog_contribution: f32,
    /// Lowest light elevation, in degrees, used for the light's transmittance to
    /// surfaces (keeps a setting sun lighting the scene); -90 is physical.
    pub transmittance_min_light_elevation: f32,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self::EARTH
    }
}

/// One of the two atmosphere lights (Unreal's atmosphere sun light index 0 or 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtmosphereLight {
    /// World direction toward the light (normalised on use).
    pub direction: Vec3,
    /// Linear illuminance above the atmosphere, in lux.
    pub lux: Vec3,
    /// Angular radius of the light's disc, in radians.
    pub radius: f32,
    /// How the light interacts with the atmosphere.
    pub settings: LightAtmosphere,
}

/// A directional light's atmosphere controls (`Light` component fields).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightAtmosphere {
    /// Linear multiplier on the light's disc luminance (Unreal's atmosphere sun disk
    /// colour scale); black hides the disc.
    pub disc_scale: [f32; 3],
    /// Attenuate the light on surfaces with the transmittance at each surface's
    /// altitude, shadowed by the planet (Unreal's per-pixel atmosphere
    /// transmittance); off, one transmittance 500 m above the ground applies
    /// everywhere, as Unreal's default.
    pub per_pixel_transmittance: bool,
}
impl Default for LightAtmosphere {
    fn default() -> Self {
        Self {
            disc_scale: [1.; 3],
            per_pixel_transmittance: false,
        }
    }
}

/// Rows in the GPU record ([`record`]; `AtmosphereModel` in the shader).
pub const RECORD_ROWS: usize = 17;
/// The record's rows of four floats.
pub type Record = [[f32; 4]; RECORD_ROWS];

impl Atmosphere {
    /// Unreal's default Earth: 6360 km ground radius, 60 km atmosphere, ground albedo
    /// sRGB 170, Rayleigh 0.0331/km with an 8 km scale height, Mie scattering
    /// 0.003996/km and absorption 0.000444/km over 1.2 km, g 0.8, ozone 0.001881/km
    /// in a tent at 25 km, 15 km wide; aerial perspective from 100 m.
    pub const EARTH: Self = Self {
        bottom_radius: 6_360_000.,
        height: 60_000.,
        ground_albedo: [0.401_978; 3],
        multi_scattering: 1.,
        trace_sample_count_scale: 1.,
        rayleigh_scattering: [5.802e-6, 13.558e-6, 33.1e-6],
        rayleigh_scale_height: 8000.,
        mie_scattering: [3.996e-6; 3],
        mie_absorption: [0.444e-6; 3],
        mie_anisotropy: 0.8,
        mie_scale_height: 1200.,
        absorption: [0.650e-6, 1.881e-6, 0.085e-6],
        absorption_tip_altitude: 25_000.,
        absorption_tip_value: 1.,
        absorption_width: 15_000.,
        sky_luminance_factor: [1.; 3],
        sky_and_aerial_perspective_luminance_factor: [1.; 3],
        aerial_perspective_view_distance_scale: 1.,
        aerial_perspective_start_depth: 100.,
        height_fog_contribution: 1.,
        transmittance_min_light_elevation: -90.,
    };

    /// Rejects values the shaders cannot represent.
    pub fn validate(&self) -> anyhow::Result<()> {
        let colour = |c: &[f32; 3]| c.iter().all(|x| x.is_finite() && *x >= 0.);
        anyhow::ensure!(
            (100.0..=1e7).contains(&self.bottom_radius)
                && (100.0..=1e7).contains(&self.height)
                && colour(&self.ground_albedo)
                && self.ground_albedo.iter().all(|x| *x <= 1.)
                && (0.0..=100.0).contains(&self.multi_scattering)
                && (0.25..=8.0).contains(&self.trace_sample_count_scale)
                && colour(&self.rayleigh_scattering)
                && colour(&self.mie_scattering)
                && colour(&self.mie_absorption)
                && colour(&self.absorption)
                && self.rayleigh_scale_height >= 1.
                && self.mie_scale_height >= 1.
                && self.rayleigh_scale_height.is_finite()
                && self.mie_scale_height.is_finite()
                && (0.0..=0.999).contains(&self.mie_anisotropy)
                && self.absorption_tip_altitude.is_finite()
                && self.absorption_tip_value >= 0.
                && self.absorption_tip_value.is_finite()
                && self.absorption_width >= 0.
                && self.absorption_width.is_finite()
                && colour(&self.sky_luminance_factor)
                && colour(&self.sky_and_aerial_perspective_luminance_factor)
                && (0.0..=1000.0).contains(&self.aerial_perspective_view_distance_scale)
                && (0.0..=1e7).contains(&self.aerial_perspective_start_depth)
                && (0.0..=16.0).contains(&self.height_fog_contribution)
                && (-90.0..=90.0).contains(&self.transmittance_min_light_elevation),
            "Invalid sky atmosphere"
        );
        Ok(())
    }

    /// Medium at `altitude` metres: (Rayleigh scattering, Mie scattering, extinction),
    /// as the shader's `atmosphereMedium`.
    pub fn medium(&self, altitude: f32) -> (Vec3, Vec3, Vec3) {
        let h = altitude.max(0.);
        let rayleigh =
            Vec3::from(self.rayleigh_scattering) * (-h / self.rayleigh_scale_height).exp();
        let density = (-h / self.mie_scale_height).exp();
        let mie = Vec3::from(self.mie_scattering) * density;
        let tent = if self.absorption_width > 0. {
            (self.absorption_tip_value
                * (1. - (h - self.absorption_tip_altitude).abs() / self.absorption_width))
                .clamp(0., 1.)
        } else {
            0.
        };
        let extinction = rayleigh
            + (Vec3::from(self.mie_scattering) + Vec3::from(self.mie_absorption)) * density
            + Vec3::from(self.absorption) * tent;
        (rayleigh, mie, extinction)
    }

    /// Transmittance from `altitude` metres toward `direction` (world, toward the
    /// light) to the top of the atmosphere, ignoring the planet (it never returns
    /// zero below the horizon), with the elevation raised to the minimum light
    /// elevation: Unreal's light transmittance for surfaces (`altitude` 500 m).
    pub fn transmittance(&self, altitude: f32, direction: Vec3) -> Vec3 {
        self.optical_path(altitude, direction, 64, 0.5)
    }

    /// The light's surface transmittance in global mode, evaluated exactly as Unreal's
    /// directional light does (`FAtmosphereSetup::GetTransmittanceAtGroundLevel`): 500 m
    /// above the ground, 15 samples at the start of each segment. That overestimates
    /// the optical depth of long slant paths (a low sun is up to a third dimmer than
    /// [`Atmosphere::transmittance`] in thick air), but converted scenes light alike.
    pub fn ground_transmittance(&self, direction: Vec3) -> Vec3 {
        self.optical_path(500., direction, 15, 0.)
    }

    /// Transmittance to the top of the atmosphere over `steps` samples placed at
    /// `offset` (0 the start, 0.5 the middle) of each segment, ignoring the planet.
    fn optical_path(&self, altitude: f32, direction: Vec3, steps: u32, offset: f32) -> Vec3 {
        let d = direction.normalize_or(Vec3::Y);
        let minimum = self.transmittance_min_light_elevation.to_radians();
        let elevation = d.y.clamp(-1., 1.).asin().max(minimum);
        let d = Vec3::new(elevation.cos(), elevation.sin(), 0.);
        let top = self.bottom_radius + self.height;
        let p = Vec3::new(0., self.bottom_radius + altitude.clamp(0., self.height), 0.);
        let b = p.dot(d);
        let q = b * b - p.length_squared() + top * top;
        let distance = (-b + q.max(0.).sqrt()).max(0.);
        let mut optical = Vec3::ZERO;
        for i in 0..steps {
            let at = p + d * (distance * (i as f32 + offset) / steps as f32);
            optical += self.medium(at.length() - self.bottom_radius).2;
        }
        (-optical * (distance / steps as f32)).exp()
    }

    /// Bits of every parameter the transmittance and multiple-scattering LUTs
    /// depend on (they rebuild when it changes).
    pub fn lut_key(&self) -> [u32; 22] {
        let mut key = [0; 22];
        let values = [
            self.bottom_radius,
            self.height,
            self.multi_scattering,
            self.trace_sample_count_scale,
            self.rayleigh_scale_height,
            self.mie_scale_height,
            self.mie_anisotropy,
            self.absorption_tip_altitude,
            self.absorption_tip_value,
            self.absorption_width,
        ]
        .into_iter()
        .chain(self.ground_albedo)
        .chain(self.rayleigh_scattering)
        .chain(
            self.mie_scattering
                .into_iter()
                .zip(self.mie_absorption)
                .map(|(s, a)| s + 2. * a),
        )
        .chain(self.absorption);
        for (slot, value) in key.iter_mut().zip(values) {
            *slot = value.to_bits();
        }
        key
    }
}

/// The GPU record (`AtmosphereModel`, `shaders/atmosphere-model.slangh`) for `model`
/// with the planet's surface at world altitude `sea_level`, the renderer's `sun` (the
/// shadowed directional light; atmosphere light 1 when `sun_is_second`, else 0) and the
/// `other` atmosphere light. Either may be absent; the sun's row is flagged (Ground w)
/// for surface lighting and shadowed scattering.
pub fn record(
    model: &Atmosphere,
    sea_level: f32,
    sun: Option<AtmosphereLight>,
    sun_is_second: bool,
    other: Option<AtmosphereLight>,
) -> Record {
    let m = model;
    let rgb = |c: [f32; 3], w: f32| [c[0], c[1], c[2], w];
    let mie_extinction = [0, 1, 2].map(|i| m.mie_scattering[i] + m.mie_absorption[i]);
    let light = |light: Option<AtmosphereLight>| match light {
        Some(l) if l.lux.is_finite() && l.direction.length_squared() > 1e-8 => {
            let direction = l.direction.normalize();
            [
                direction.extend(l.radius.max(0.)).to_array(),
                l.lux
                    .max(Vec3::ZERO)
                    .extend(f32::from(u8::from(l.settings.per_pixel_transmittance)))
                    .to_array(),
                m.ground_transmittance(direction).extend(1.).to_array(),
                rgb(l.settings.disc_scale.map(|c| c.max(0.)), 1.),
            ]
        }
        // An absent light scatters nothing; a sun outside the atmosphere still lights
        // surfaces unattenuated.
        _ => [[0., 1., 0., 0.], [0.; 4], [1., 1., 1., 0.], [0.; 4]],
    };
    let [mut l0, mut l1] = [sun, other].map(light);
    if sun_is_second {
        std::mem::swap(&mut l0, &mut l1);
    }
    // Only the sun's row carries the flag (other lights are never on surfaces).
    let other_row = if sun_is_second { &mut l0 } else { &mut l1 };
    other_row[2][3] = 0.;
    [
        [
            m.bottom_radius,
            m.bottom_radius + m.height,
            m.multi_scattering,
            m.mie_anisotropy,
        ],
        rgb(m.rayleigh_scattering, -1. / m.rayleigh_scale_height),
        rgb(m.mie_scattering, -1. / m.mie_scale_height),
        rgb(mie_extinction, m.trace_sample_count_scale),
        rgb(m.absorption, m.absorption_tip_altitude),
        [
            m.absorption_tip_value,
            m.absorption_width,
            m.transmittance_min_light_elevation.to_radians().sin(),
            m.aerial_perspective_start_depth,
        ],
        rgb(m.ground_albedo, m.aerial_perspective_view_distance_scale),
        rgb(m.sky_luminance_factor, m.height_fog_contribution),
        rgb(m.sky_and_aerial_perspective_luminance_factor, sea_level),
        l0[0],
        l0[1],
        l0[2],
        l0[3],
        l1[0],
        l1[1],
        l1[2],
        l1[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earth_matches_unreal_defaults() {
        let e = Atmosphere::EARTH;
        // Unreal stores colour x scale: Rayleigh (0.175287, 0.409607, 1) x 0.0331/km.
        let rayleigh = [0.175_287, 0.409_607, 1.].map(|c| c * 0.0331e-3);
        for (a, b) in e.rayleigh_scattering.iter().zip(rayleigh) {
            assert!((a - b).abs() < 1e-9, "{a} {b}");
        }
        let ozone = [0.345_561, 1., 0.045_189].map(|c| c * 0.001_881e-3);
        for (a, b) in e.absorption.iter().zip(ozone) {
            assert!((a - b).abs() < 1e-9, "{a} {b}");
        }
        e.validate().expect("Earth is valid");
    }

    #[test]
    fn noon_transmittance_is_bright_and_sunset_is_red() {
        let e = Atmosphere::EARTH;
        let noon = e.ground_transmittance(Vec3::Y);
        assert!(noon.x > 0.9 && noon.z > 0.6 && noon.z < noon.x, "{noon}");
        // Unreal's 15 start-of-segment samples, not the exact integral (0.945, 0.875, 0.776).
        assert!(
            (noon - Vec3::new(0.9258, 0.8442, 0.7199))
                .abs()
                .max_element()
                < 2e-3,
            "{noon}"
        );
        let exact = e.transmittance(500., Vec3::Y);
        assert!(
            (exact - Vec3::new(0.9449, 0.8751, 0.7763))
                .abs()
                .max_element()
                < 2e-3,
            "{exact}"
        );
        let low = e.ground_transmittance(Vec3::new(1., 2f32.to_radians().tan(), 0.));
        assert!(low.x > 0.1 && low.z < low.x * 0.1, "{low}");
        // Below the horizon Unreal's global transmittance still passes light.
        let below = e.ground_transmittance(Vec3::new(1., -0.02, 0.));
        assert!(below.x > 0. && below.x < low.x, "{below}");
        // The minimum elevation keeps the light at that elevation's colour.
        let held = Atmosphere {
            transmittance_min_light_elevation: 10.,
            ..e
        };
        let ten = e.ground_transmittance(Vec3::new(1., 10f32.to_radians().tan(), 0.));
        assert!((held.ground_transmittance(Vec3::new(1., -0.3, 0.)) - ten).length() < 1e-5);
    }

    #[test]
    fn record_packs_the_shader_rows() {
        let sun = AtmosphereLight {
            direction: Vec3::new(0., 2., 0.),
            lux: Vec3::splat(100_000.),
            radius: 0.00465,
            settings: LightAtmosphere::default(),
        };
        let r = record(&Atmosphere::EARTH, 5., Some(sun), false, None);
        assert_eq!(r[0][1], 6_420_000.);
        assert!((r[1][3] + 1. / 8000.).abs() < 1e-9);
        assert_eq!(r[8][3], 5.);
        assert_eq!(r[9], [0., 1., 0., 0.00465]);
        assert_eq!((r[11][3], r[12][3]), (1., 1.));
        assert_eq!(r[14], [0.; 4]);
        // A moon in the sun slot: atmosphere light 1, flagged; the set sun is light 0.
        let night = record(&Atmosphere::EARTH, 0., Some(sun), true, Some(sun));
        assert_eq!((night[11][3], night[15][3]), (0., 1.));
    }
}
