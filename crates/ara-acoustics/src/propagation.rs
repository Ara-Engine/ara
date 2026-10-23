//! Distance curves, air absorption and Doppler: free-field propagation per source.

use crate::math::{V3, dot, normalize_or, sub};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Per-source acoustic behaviour, alongside `AudioSource`. Optional: sources without it
/// use `AcousticSource::default()`.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::component::Component))]
#[ara(component, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticSource {
    /// Trace geometry between source and listener for occlusion and transmission.
    #[ara(default = true)]
    pub occlusion: bool,
    /// Radius sampled for partial (volumetric) occlusion; 0 traces only the center ray.
    #[ara(unit = Meters, default = 0.5, range(min = 0, max = 50))]
    pub occlusion_radius: f32,
    /// Apply frequency-dependent air absorption over the propagation distance.
    #[ara(default = true)]
    pub air_absorption: bool,
    /// Doppler strength: 0 disables pitch shift, 1 is physical, larger exaggerates.
    #[ara(default = 1, range(min = 0, max = 4))]
    pub doppler: f32,
    /// Room reverb send before zone and portal attenuation.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub reverb_send: f32,
    /// Custom distance curve: `[meters, linear gain]` points with increasing distance.
    /// Empty uses `AudioSource.attenuation`; cones still apply.
    pub distance_curve: Vec<[f32; 2]>,
}

impl Default for AcousticSource {
    fn default() -> Self {
        Self {
            occlusion: true,
            occlusion_radius: 0.5,
            air_absorption: true,
            doppler: 1.0,
            reverb_send: 1.0,
            distance_curve: Vec::new(),
        }
    }
}

/// World-wide propagation constants, stored in project/world settings.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticsSettings {
    /// Speed of sound, for Doppler and reflection delays.
    #[ara(unit = MetersPerSecond, default = 343, range(min = 1, max = 2000))]
    pub speed_of_sound: f32,
    /// Air absorption per meter for low/mid/high bands: gain = exp(-coefficient × distance).
    #[ara(default = [0.0002, 0.0017, 0.0182], range(min = 0, max = 1))]
    pub air_coefficients: [f32; 3],
    /// Rays per source for volumetric occlusion, including the center ray.
    #[ara(default = 9, range(min = 1, max = 64))]
    pub occlusion_rays: u32,
    /// Maximum portals sound may pass through between zones.
    #[ara(default = 3, range(min = 0, max = 8))]
    pub max_portal_depth: u32,
}

impl Default for AcousticsSettings {
    fn default() -> Self {
        Self {
            speed_of_sound: 343.0,
            air_coefficients: [0.0002, 0.0017, 0.0182],
            occlusion_rays: 9,
            max_portal_depth: 3,
        }
    }
}

impl AcousticsSettings {
    /// True when every value is finite and in range.
    pub fn is_valid(&self) -> bool {
        self.speed_of_sound.is_finite()
            && (1.0..=2000.0).contains(&self.speed_of_sound)
            && self
                .air_coefficients
                .iter()
                .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
            && (1..=64).contains(&self.occlusion_rays)
            && self.max_portal_depth <= 8
    }

    /// Low/mid/high air absorption gains over `distance` meters.
    pub fn air(&self, distance: f32) -> [f32; 3] {
        self.air_coefficients
            .map(|c| (-c * distance.max(0.0)).exp().clamp(0.0, 1.0))
    }

    /// Doppler playback-rate factor for the relative motion along the line of sight,
    /// with `strength` scaling both velocities. Clamped to [0.25, 4].
    pub fn doppler(&self, strength: f32, source: (V3, V3), listener: (V3, V3)) -> f32 {
        if strength == 0.0 {
            return 1.0;
        }
        let c = self.speed_of_sound;
        let toward_listener = normalize_or(sub(listener.0, source.0), [0.0; 3]);
        let limit = c * 0.9;
        let vs = (dot(source.1, toward_listener) * strength).clamp(-limit, limit);
        let vl = (dot(listener.1, toward_listener) * strength).clamp(-limit, limit);
        // f' = f (c - v_listener·u) / (c - v_source·u), u pointing source → listener.
        ((c - vl) / (c - vs)).clamp(0.25, 4.0)
    }
}

/// Evaluates a custom distance curve; outside the points it holds the end values.
/// Returns None for an empty or invalid curve.
pub fn curve_gain(points: &[[f32; 2]], distance: f32) -> Option<f32> {
    let valid = !points.is_empty()
        && points.iter().all(|p| {
            p[0].is_finite() && p[1].is_finite() && p[0] >= 0.0 && (0.0..=16.0).contains(&p[1])
        })
        && points.windows(2).all(|w| w[0][0] < w[1][0]);
    if !valid {
        return None;
    }
    let first = points[0];
    let last = points[points.len() - 1];
    if distance <= first[0] {
        return Some(first[1]);
    }
    if distance >= last[0] {
        return Some(last[1]);
    }
    let i = points.partition_point(|p| p[0] <= distance);
    let (a, b) = (points[i - 1], points[i]);
    let t = (distance - a[0]) / (b[0] - a[0]);
    Some(a[1] + (b[1] - a[1]) * t)
}
