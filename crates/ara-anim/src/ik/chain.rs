// Schema defaults accept literals; these match f32::consts::PI exactly.
#![allow(clippy::approx_constant)]
use crate::{Error, Pose, Skeleton, ensure, pose::model_matrices};
use ara_schema::Schema;
use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Angular constraint relative to the local bind rotation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum LimitKind {
    /// Unrestricted rotation.
    #[default]
    Free,
    /// Circular swing cone plus signed twist interval.
    Cone,
    /// Only rotation about the hinge axis is permitted.
    Hinge,
}

/// Anatomical angular bounds, in radians around a unit bind-local axis.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct JointLimit {
    /// Constraint primitive.
    pub kind: LimitKind,
    /// Unit hinge or twist axis in the joint's bind-local frame.
    #[ara(default = [1, 0, 0])]
    pub axis: [f32; 3],
    /// Maximum swing angle for a cone.
    #[ara(default = 3.1415927)]
    pub swing: f32,
    /// Minimum signed twist or hinge angle.
    #[ara(default = -3.1415927)]
    pub min: f32,
    /// Maximum signed twist or hinge angle.
    #[ara(default = 3.1415927)]
    pub max: f32,
}
impl Default for JointLimit {
    fn default() -> Self {
        Self {
            kind: LimitKind::Free,
            axis: [1.0, 0.0, 0.0],
            swing: std::f32::consts::PI,
            min: -std::f32::consts::PI,
            max: std::f32::consts::PI,
        }
    }
}
impl JointLimit {
    pub(crate) fn valid(&self) -> bool {
        let axis = Vec3::from(self.axis);
        axis.is_finite()
            && (axis.length_squared() - 1.0).abs() < 1e-4
            && self.swing.is_finite()
            && (0.0..=std::f32::consts::PI).contains(&self.swing)
            && self.min.is_finite()
            && self.max.is_finite()
            && self.min >= -std::f32::consts::PI
            && self.max <= std::f32::consts::PI
            && self.min <= self.max
    }
    /// Projects a normalized bind-relative quaternion into the allowed region.
    pub fn project(&self, rotation: Quat) -> Result<Quat, Error> {
        ensure(
            self.valid() && rotation.is_finite() && rotation.is_normalized(),
            "invalid joint limit",
        )?;
        Ok(self.clamp(rotation))
    }
    fn clamp(&self, rotation: Quat) -> Quat {
        if self.kind == LimitKind::Free {
            return rotation;
        }
        let q = if rotation.w < 0.0 {
            -rotation
        } else {
            rotation
        };
        let axis = Vec3::from(self.axis);
        let projected = axis * Vec3::new(q.x, q.y, q.z).dot(axis);
        let raw = Quat::from_xyzw(projected.x, projected.y, projected.z, q.w);
        let twist = if raw.length_squared() > 1e-12 {
            raw.normalize()
        } else {
            Quat::IDENTITY
        };
        let signed = 2.0
            * Vec3::new(twist.x, twist.y, twist.z)
                .dot(axis)
                .atan2(twist.w);
        let limited_twist = Quat::from_axis_angle(axis, signed.clamp(self.min, self.max));
        if self.kind == LimitKind::Hinge {
            return limited_twist;
        }
        let swing = (q * twist.conjugate()).normalize();
        let angle = 2.0 * swing.w.abs().clamp(0.0, 1.0).acos();
        let swing = if angle > self.swing && angle > 1e-6 {
            Quat::IDENTITY.slerp(swing, self.swing / angle)
        } else {
            swing
        };
        (swing * limited_twist).normalize()
    }
}

/// Canonical chain setup, addressed by stable skeleton joint IDs.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainData {
    /// Consecutive parent-to-child joints, including the end effector.
    pub joints: Vec<String>,
    /// One limit per rotating joint; empty uses unrestricted limits.
    pub limits: Vec<JointLimit>,
}
/// Iterative algorithm used for a cooked chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainSolver {
    /// Forward and backward reaching with angular projection after every pass.
    Fabrik,
    /// Cyclic coordinate descent, tip to root.
    Ccd,
}
/// Fixed work bound and convergence criterion; never uses wall-clock time.
#[derive(Debug, Clone, Copy)]
pub struct SolverSettings {
    /// Maximum iteration count.
    pub iterations: u32,
    /// End-effector distance tolerance in meters.
    pub tolerance: f32,
}
impl Default for SolverSettings {
    fn default() -> Self {
        Self {
            iterations: 24,
            tolerance: 0.0001,
        }
    }
}
/// Residual and deterministic amount of solver work.
#[derive(Debug, Clone, Copy)]
pub struct SolveReport {
    /// Distance from solved tip to requested target in meters.
    pub error: f32,
    /// Iterations actually executed.
    pub iterations: u32,
}
/// Validated chain and actor-local preallocated scratch. Construct outside ticks.
#[derive(Debug)]
pub struct Chain {
    skeleton: u64,
    joints: Vec<usize>,
    limits: Vec<JointLimit>,
    model: Vec<Mat4>,
    positions: Vec<Vec3>,
    lengths: Vec<f32>,
}
impl Chain {
    /// Validates stable IDs, consecutive topology and angular bounds.
    pub fn cook(skeleton: &Skeleton, data: &ChainData) -> Result<Self, Error> {
        ensure(
            data.joints.len() >= 2
                && (data.limits.is_empty() || data.limits.len() + 1 == data.joints.len())
                && data.limits.iter().all(JointLimit::valid),
            "invalid IK chain size or limits",
        )?;
        let joints: Vec<_> = data
            .joints
            .iter()
            .map(|id| {
                skeleton
                    .data
                    .joints
                    .iter()
                    .position(|j| &j.id == id)
                    .ok_or_else(|| Error(format!("IK joint {id} does not exist")))
            })
            .collect::<Result<_, _>>()?;
        ensure(
            joints
                .windows(2)
                .all(|j| skeleton.parents[j[1]] == Some(j[0] as u32)),
            "IK chain must follow consecutive parents",
        )?;
        let n = joints.len();
        let mut out = Self {
            skeleton: skeleton.signature(),
            joints,
            limits: if data.limits.is_empty() {
                vec![JointLimit::default(); n - 1]
            } else {
                data.limits.clone()
            },
            model: vec![Mat4::IDENTITY; skeleton.len()],
            positions: vec![Vec3::ZERO; n],
            lengths: vec![0.0; n - 1],
        };
        model_matrices(skeleton, skeleton.bind_pose(), &mut out.model)?;
        ensure(
            out.joints
                .windows(2)
                .all(|j| out.model[j[0]].w_axis.distance(out.model[j[1]].w_axis) > 1e-6),
            "IK segments must have nonzero length",
        )?;
        Ok(out)
    }
    /// Root and tip indices, resolved once from stable IDs.
    pub fn endpoints(&self) -> (usize, usize) {
        (self.joints[0], self.joints[self.joints.len() - 1])
    }
    /// Solves without allocating; invalid arguments leave the pose unchanged.
    pub fn solve(
        &mut self,
        skeleton: &Skeleton,
        pose: &mut Pose,
        target: Vec3,
        solver: ChainSolver,
        settings: SolverSettings,
    ) -> Result<SolveReport, Error> {
        ensure(
            self.skeleton == skeleton.signature()
                && pose.len() == skeleton.len()
                && target.is_finite()
                && settings.iterations > 0
                && settings.iterations <= 1024
                && settings.tolerance.is_finite()
                && settings.tolerance >= 0.0,
            "invalid IK solve input",
        )?;
        model_matrices(skeleton, pose, &mut self.model)?;
        ensure(
            self.model
                .iter()
                .all(|m| m.is_finite() && m.determinant().abs() > 1e-12),
            "invalid IK pose",
        )?;
        for (i, j) in self.joints.iter().enumerate() {
            self.positions[i] = self.model[*j].w_axis.truncate();
        }
        for i in 0..self.lengths.len() {
            self.lengths[i] = self.positions[i].distance(self.positions[i + 1]);
        }
        ensure(
            self.lengths.iter().all(|l| *l > 1e-6),
            "collapsed IK segment",
        )?;
        // Enforce limits even when the tip initially happens to be at the target.
        for i in 0..self.limits.len() {
            let j = self.joints[i];
            let rest = skeleton.bind.rotations[j];
            pose.rotations[j] =
                (rest * self.limits[i].clamp(rest.conjugate() * pose.rotations[j])).normalize();
        }
        model_matrices(skeleton, pose, &mut self.model)?;
        let last = self.joints.len() - 1;
        let mut report = SolveReport {
            error: self.model[self.joints[last]]
                .w_axis
                .truncate()
                .distance(target),
            iterations: 0,
        };
        for pass in 0..settings.iterations {
            if report.error <= settings.tolerance {
                break;
            }
            if solver == ChainSolver::Fabrik {
                for (i, j) in self.joints.iter().enumerate() {
                    self.positions[i] = self.model[*j].w_axis.truncate();
                }
                let anchor = self.positions[0];
                self.positions[last] = target;
                for i in (0..last).rev() {
                    self.positions[i] = self.positions[i + 1]
                        + direction(self.positions[i] - self.positions[i + 1]) * self.lengths[i];
                }
                self.positions[0] = anchor;
                for i in 0..last {
                    self.positions[i + 1] = self.positions[i]
                        + direction(self.positions[i + 1] - self.positions[i]) * self.lengths[i];
                }
                for i in 0..last {
                    self.rotate(skeleton, pose, i, self.positions[i + 1], self.joints[i + 1])?;
                }
            } else {
                for i in (0..last).rev() {
                    self.rotate(skeleton, pose, i, target, self.joints[last])?;
                }
            }
            report = SolveReport {
                error: self.model[self.joints[last]]
                    .w_axis
                    .truncate()
                    .distance(target),
                iterations: pass + 1,
            };
        }
        Ok(report)
    }
    fn rotate(
        &mut self,
        skeleton: &Skeleton,
        pose: &mut Pose,
        index: usize,
        target: Vec3,
        tip: usize,
    ) -> Result<(), Error> {
        let j = self.joints[index];
        let center = self.model[j].w_axis.truncate();
        let from = self.model[tip].w_axis.truncate() - center;
        let to = target - center;
        if from.length_squared() < 1e-12 || to.length_squared() < 1e-12 {
            return Ok(());
        }
        let delta = Quat::from_rotation_arc(from.normalize(), to.normalize());
        let rotation = delta * self.model[j].to_scale_rotation_translation().1;
        let parent = skeleton.parents[j].map_or(Quat::IDENTITY, |p| {
            self.model[p as usize].to_scale_rotation_translation().1
        });
        let rest = skeleton.bind.rotations[j];
        pose.rotations[j] = (rest
            * self.limits[index].clamp(rest.conjugate() * parent.conjugate() * rotation))
        .normalize();
        model_matrices(skeleton, pose, &mut self.model)
    }
}
fn direction(v: Vec3) -> Vec3 {
    v.try_normalize().unwrap_or(Vec3::X)
}
