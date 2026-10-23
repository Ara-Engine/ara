use crate::{Error, ensure};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Exact critically damped spring state. Store it with the actor for rollback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    /// Smoothed vector value.
    pub value: Vec3,
    /// Derivative in units per second.
    pub velocity: Vec3,
}
impl Spring {
    /// Integrates toward a constant target over the supplied fixed interval.
    /// `half_life` controls exponential decay; no frame time or clock is read.
    pub fn step(&mut self, target: Vec3, half_life: f32, dt: f32) -> Result<(), Error> {
        ensure(
            target.is_finite()
                && self.value.is_finite()
                && self.velocity.is_finite()
                && half_life.is_finite()
                && half_life > 0.0
                && dt.is_finite()
                && dt >= 0.0,
            "invalid spring input",
        )?;
        let w = std::f32::consts::LN_2 / half_life;
        let offset = self.value - target;
        let c = self.velocity + offset * w;
        let decay = (-w * dt).exp();
        self.value = target + (offset + c * dt) * decay;
        self.velocity = (self.velocity - c * (w * dt)) * decay;
        Ok(())
    }
}
/// Character locomotion predictor; position/velocity are authoritative actor state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryPredictor {
    /// World-space character position.
    pub position: Vec3,
    /// Smoothed world-space linear velocity and acceleration.
    pub motion: Spring,
    /// Character world-space facing yaw, in radians.
    pub yaw: f32,
    /// Yaw velocity in radians per second.
    pub yaw_velocity: f32,
}
/// A future root sample in the actor's current local frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrajectoryPoint {
    /// Root displacement in meters.
    pub position: Vec3,
    /// Unit local facing vector.
    pub facing: Vec3,
}
impl TrajectoryPredictor {
    /// Advances one fixed tick from desired world velocity and yaw.
    pub fn step(
        &mut self,
        desired_velocity: Vec3,
        desired_yaw: f32,
        half_life: f32,
        dt: f32,
    ) -> Result<(), Error> {
        ensure(
            desired_yaw.is_finite()
                && self.yaw.is_finite()
                && self.yaw_velocity.is_finite()
                && self.position.is_finite(),
            "invalid trajectory input",
        )?;
        let mut motion = self.motion;
        let previous = motion.value;
        motion.step(desired_velocity, half_life, dt)?;
        let delta = (desired_yaw - self.yaw + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let mut yaw = Spring {
            value: Vec3::new(self.yaw, 0.0, 0.0),
            velocity: Vec3::new(self.yaw_velocity, 0.0, 0.0),
        };
        yaw.step(Vec3::new(self.yaw + delta, 0.0, 0.0), half_life, dt)?;
        self.position += (previous + motion.value) * (dt * 0.5);
        self.motion = motion;
        self.yaw = yaw.value.x;
        self.yaw_velocity = yaw.velocity.x;
        Ok(())
    }
    /// Predicts ordered future horizons into caller storage without changing live state.
    /// Prediction uses the identical spring update, integrating in bounded fixed substeps.
    pub fn predict(
        &self,
        desired_velocity: Vec3,
        desired_yaw: f32,
        half_life: f32,
        horizons: &[f32],
        fixed_dt: f32,
        out: &mut [TrajectoryPoint],
    ) -> Result<(), Error> {
        ensure(
            out.len() == horizons.len()
                && fixed_dt.is_finite()
                && fixed_dt > 0.0
                && horizons
                    .iter()
                    .all(|t| t.is_finite() && *t >= 0.0 && *t <= 10.0)
                && horizons.windows(2).all(|t| t[0] <= t[1]),
            "invalid trajectory horizons",
        )?;
        ensure(
            horizons.last().is_none_or(|t| *t / fixed_dt <= 4096.0),
            "trajectory prediction exceeds its substep budget",
        )?;
        let mut state = *self;
        let mut elapsed = 0.0;
        let inverse = Quat::from_rotation_y(-self.yaw);
        for (horizon, point) in horizons.iter().zip(out) {
            while elapsed < *horizon {
                let dt = (*horizon - elapsed).min(fixed_dt);
                state.step(desired_velocity, desired_yaw, half_life, dt)?;
                elapsed += dt;
            }
            *point = TrajectoryPoint {
                position: inverse * (state.position - self.position),
                facing: inverse * (Quat::from_rotation_y(state.yaw) * Vec3::Z),
            };
        }
        Ok(())
    }
}
