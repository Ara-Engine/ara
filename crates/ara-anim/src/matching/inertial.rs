use super::Spring;
use crate::{Error, Pose, ensure};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
struct Offset {
    translation: Spring,
    rotation: Spring,
    scale: Spring,
}
/// Velocity-preserving pose offsets with exact critically damped decay.
/// Unlike cross-fades, only the incoming animation is evaluated after a transition.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Inertializer {
    offsets: Vec<Offset>,
    half_life: f32,
}
impl Clone for Inertializer {
    fn clone(&self) -> Self {
        Self {
            offsets: self.offsets.clone(),
            half_life: self.half_life,
        }
    }
    fn clone_from(&mut self, other: &Self) {
        self.offsets.clone_from(&other.offsets);
        self.half_life = other.half_life;
    }
}
impl Inertializer {
    pub(crate) fn valid(&self, joints: usize) -> bool {
        self.offsets.len() == joints
            && self.half_life.is_finite()
            && self.half_life > 0.0
            && self.offsets.iter().all(|o| {
                [o.translation, o.rotation, o.scale]
                    .iter()
                    .all(|s| s.value.is_finite() && s.velocity.is_finite())
            })
    }
    /// Allocates one offset/derivative triplet per joint outside the tick loop.
    pub fn new(joints: usize, half_life: f32) -> Result<Self, Error> {
        ensure(
            half_life.is_finite() && half_life > 0.0,
            "invalid inertialization half-life",
        )?;
        Ok(Self {
            offsets: vec![Offset::default(); joints],
            half_life,
        })
    }
    /// Captures C0/C1 local translation, quaternion-log and log-scale differences.
    pub fn begin(
        &mut self,
        previous_out: &Pose,
        current_out: &Pose,
        previous_in: &Pose,
        current_in: &Pose,
        dt: f32,
    ) -> Result<(), Error> {
        let n = self.offsets.len();
        ensure(
            [previous_out, current_out, previous_in, current_in]
                .iter()
                .all(|p| p.len() == n)
                && dt.is_finite()
                && dt > 0.0,
            "invalid inertialization poses or interval",
        )?;
        for (i, offset) in self.offsets.iter_mut().enumerate() {
            let delta = |a: &Pose, b: &Pose| -> (Vec3, Vec3, Vec3) {
                let q = a.rotations[i] * b.rotations[i].conjugate();
                let q = if q.w < 0.0 { -q } else { q };
                (
                    (a.translations[i] - b.translations[i]).into(),
                    q.to_scaled_axis(),
                    (Vec3::from(a.scales[i]) / Vec3::from(b.scales[i])).ln(),
                )
            };
            let (t, r, s) = delta(current_out, current_in);
            let (pt, pr, ps) = delta(previous_out, previous_in);
            *offset = Offset {
                translation: Spring {
                    value: t,
                    velocity: (t - pt) / dt,
                },
                rotation: Spring {
                    value: r,
                    velocity: (r - pr) / dt,
                },
                scale: Spring {
                    value: s,
                    velocity: (s - ps) / dt,
                },
            };
        }
        Ok(())
    }
    /// Advances offsets, then applies them to the incoming pose in place.
    /// Pass zero immediately after begin to reproduce the outgoing pose exactly.
    pub fn apply(&mut self, incoming: &mut Pose, dt: f32) -> Result<(), Error> {
        ensure(
            incoming.len() == self.offsets.len() && dt.is_finite() && dt >= 0.0,
            "invalid inertialization output",
        )?;
        let w = std::f32::consts::LN_2 / self.half_life;
        let decay = (-w * dt).exp();
        for (i, offset) in self.offsets.iter_mut().enumerate() {
            if *offset == Offset::default() {
                continue;
            }
            for spring in [
                &mut offset.translation,
                &mut offset.rotation,
                &mut offset.scale,
            ] {
                let c = spring.velocity + spring.value * w;
                spring.value = (spring.value + c * dt) * decay;
                spring.velocity = (spring.velocity - c * (w * dt)) * decay;
                if spring.value.length_squared() + spring.velocity.length_squared() < 1e-14 {
                    *spring = Spring::default();
                }
            }
            incoming.translations[i] += glam::Vec3A::from(offset.translation.value);
            if offset.rotation.value != Vec3::ZERO {
                incoming.rotations[i] = (Quat::from_scaled_axis(offset.rotation.value)
                    * incoming.rotations[i])
                    .normalize();
            }
            if offset.scale.value != Vec3::ZERO {
                incoming.scales[i] *= glam::Vec3A::from(offset.scale.value.exp());
            }
        }
        Ok(())
    }
    /// Clears a transition at a fixed boundary without allocation.
    pub fn reset(&mut self) {
        self.offsets.fill(Offset::default());
    }
}
