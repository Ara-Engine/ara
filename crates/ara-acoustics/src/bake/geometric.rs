//! Built-in deterministic baker: stochastic ray tracing for room statistics, then
//! Eyring reverberation times. Needs no native library, so cells always cook.

use super::{ReverbBaker, ReverbProbe};
use crate::{
    geometry::MeshScene,
    math::{V3, add, cross, dot, fibonacci, normalize_or, scale, sub},
    propagation::AcousticsSettings,
};
use ara_audio::{AudioError, Result};

/// Ray-traced statistics baker. The same scene, positions and seed give identical probes.
#[derive(Debug, Clone, Copy)]
pub struct GeometricBaker {
    /// Rays per probe.
    pub rays: u32,
    /// Maximum reflections followed per ray.
    pub bounces: u32,
    /// Distance beyond which a ray counts as escaping to open space, in meters.
    pub max_distance: f32,
    /// Seed for diffuse reflection directions.
    pub seed: u64,
    /// Air absorption and speed of sound.
    pub settings: AcousticsSettings,
}

impl Default for GeometricBaker {
    fn default() -> Self {
        Self {
            rays: 1024,
            bounces: 24,
            max_distance: 500.0,
            seed: 0x5eed_a5a5,
            settings: AcousticsSettings::default(),
        }
    }
}

/// SplitMix64: tiny, deterministic and well distributed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Cosine-weighted direction in the hemisphere around `normal`.
fn diffuse(normal: V3, rng: &mut Rng) -> V3 {
    let (u, v) = (rng.next(), rng.next());
    let r = u.sqrt();
    let phi = std::f32::consts::TAU * v;
    let helper = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let t = normalize_or(cross(helper, normal), [1.0, 0.0, 0.0]);
    let b = cross(normal, t);
    normalize_or(
        add(
            add(scale(t, r * phi.cos()), scale(b, r * phi.sin())),
            scale(normal, (1.0 - u).max(0.0).sqrt()),
        ),
        normal,
    )
}

impl GeometricBaker {
    fn probe(&self, scene: &MeshScene, position: V3, index: usize) -> ReverbProbe {
        let n = self.rays as usize;
        let mut rng = Rng(self.seed ^ (index as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
        let mut volume = 0.0_f32;
        let mut path = 0.0_f32;
        let mut segments = 0_u32;
        let mut escapes = 0_u32;
        let mut absorbed = [0.0_f32; 3];
        let mut reflections = 0_u32;
        for i in 0..n {
            let mut origin = position;
            let mut direction = fibonacci(i, n);
            for bounce in 0..self.bounces {
                let Some(hit) = scene.closest(origin, direction, self.max_distance) else {
                    escapes += 1;
                    if bounce == 0 {
                        volume += self.max_distance.powi(3);
                    }
                    break;
                };
                if bounce == 0 {
                    volume += hit.distance.powi(3);
                }
                path += hit.distance;
                segments += 1;
                reflections += 1;
                for (sum, a) in absorbed.iter_mut().zip(hit.material.absorption) {
                    *sum += a;
                }
                let point = add(origin, scale(direction, hit.distance));
                let specular = sub(
                    direction,
                    scale(hit.normal, 2.0 * dot(direction, hit.normal)),
                );
                direction = if rng.next() < hit.material.scattering {
                    diffuse(hit.normal, &mut rng)
                } else {
                    specular
                };
                origin = add(point, scale(hit.normal, 1e-3));
            }
        }
        // Visible volume: each ray owns a 4π/N solid-angle cone of length r.
        let volume = (4.0 * std::f32::consts::PI / n as f32 * volume / 3.0).max(1.0);
        let mean_free_path = if segments == 0 {
            self.max_distance
        } else {
            path / segments as f32
        };
        let total = (reflections + escapes).max(1) as f32;
        let escape = escapes as f32 / total;
        let absorption = absorbed.map(|a| {
            let surface = if reflections == 0 {
                1.0
            } else {
                a / reflections as f32
            };
            (1.0 - (1.0 - surface) * (1.0 - escape)).clamp(0.0, 1.0)
        });
        let rt60 = std::array::from_fn(|b| {
            let alpha = absorption[b].clamp(1e-4, 0.9999);
            let air = 2.0 * self.settings.air_coefficients[b] * mean_free_path;
            (0.161 * mean_free_path / 4.0 / (-(1.0 - alpha).ln() + air)).clamp(0.05, 20.0)
        });
        ReverbProbe {
            position,
            rt60,
            mean_free_path,
            volume,
            absorption,
            escape: escapes as f32 / n as f32,
        }
    }
}

impl ReverbBaker for GeometricBaker {
    fn name(&self) -> String {
        "ara-geometric/1".into()
    }

    fn bake(&mut self, scene: &MeshScene, positions: &[V3]) -> Result<Vec<ReverbProbe>> {
        if self.rays == 0
            || self.bounces == 0
            || self.max_distance.is_nan()
            || self.max_distance <= 0.0
            || !self.settings.is_valid()
        {
            return Err(AudioError::Invalid("geometric baker settings"));
        }
        let mut scene = scene.clone();
        scene.commit();
        Ok(positions
            .iter()
            .enumerate()
            .map(|(i, p)| self.probe(&scene, *p, i))
            .collect())
    }
}
