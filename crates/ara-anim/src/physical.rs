//! Powered ragdoll targets and fixed-tick animation/ragdoll/get-up blending.
use crate::{
    Error, Pose, RootMotion, Skeleton, ensure,
    pose::{model_matrices, nlerp},
};
use ara_schema::Schema;
use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Position and orientation spring gains in SI units.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MotorGains {
    /// Linear spring stiffness in N/m.
    #[ara(default = 300)]
    pub position: f32,
    /// Linear damping in N*s/m.
    #[ara(default = 35)]
    pub linear_damping: f32,
    /// Angular spring stiffness in N*m/rad.
    #[ara(default = 80)]
    pub rotation: f32,
    /// Angular damping in N*m*s/rad.
    #[ara(default = 12)]
    pub angular_damping: f32,
    /// Maximum applied linear force in N.
    #[ara(default = 1000)]
    pub max_force: f32,
    /// Maximum applied torque in N*m.
    #[ara(default = 250)]
    pub max_torque: f32,
}
impl Default for MotorGains {
    fn default() -> Self {
        Self {
            position: 300.0,
            linear_damping: 35.0,
            rotation: 80.0,
            angular_damping: 12.0,
            max_force: 1000.0,
            max_torque: 250.0,
        }
    }
}
/// One capsule and parent constraint in a canonical powered-ragdoll asset.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RagdollBone {
    /// Stable skeleton joint ID.
    pub joint: String,
    /// Parent body index; absent for a free root. Parents must precede children.
    pub parent: Option<u32>,
    /// Body transform relative to the skeleton joint.
    pub offset: crate::data::AnimTransform,
    /// Capsule radius in meters.
    #[ara(default = 0.08)]
    pub radius: f32,
    /// Capsule cylinder half-height in meters.
    #[ara(default = 0.15)]
    pub half_height: f32,
    /// Body mass in kg.
    #[ara(default = 3)]
    pub mass: f32,
    /// Bind-local angular constraint for the parent/body connection.
    pub limit: crate::ik::JointLimit,
    /// Bounded spring gains.
    pub motor: MotorGains,
}
impl Default for RagdollBone {
    fn default() -> Self {
        Self {
            joint: String::new(),
            parent: None,
            offset: Default::default(),
            radius: 0.08,
            half_height: 0.15,
            mass: 3.0,
            limit: Default::default(),
            motor: Default::default(),
        }
    }
}
/// Canonical body/constraint/motor configuration, cooked at import or spawn boundaries.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RagdollProfile {
    /// Bodies in stable parent-before-child order.
    pub bones: Vec<RagdollBone>,
}
/// Fixed-tick physical-animation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum PhysicalMode {
    /// Animation target with powered bodies.
    #[default]
    Animated,
    /// Temporary partial ragdoll, preserving active drives.
    HitReaction,
    /// Freely simulated ragdoll.
    Ragdoll,
    /// Powered recovery and blend toward a caller-selected get-up animation.
    GettingUp,
    /// Freely simulated terminal pose; recovery requires an explicit new request.
    Dead,
}
/// Complete future-affecting blend state. Store this in the character's ECS component.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PhysicalState {
    /// Requested mode.
    pub mode: PhysicalMode,
    /// Current physical pose contribution in [0,1].
    pub weight: f32,
    /// Start contribution for this transition.
    pub start: f32,
    /// Target contribution.
    pub target: f32,
    /// Completed transition ticks.
    pub elapsed: u32,
    /// Fixed transition duration in ticks.
    pub duration: u32,
}
impl PhysicalState {
    /// Starts an explicit hit/death/recovery intent at a fixed simulation boundary.
    /// Partial hit reactions use the supplied [0,1] contribution; death/ragdoll use one.
    pub fn request(
        &mut self,
        mode: PhysicalMode,
        ticks: u32,
        reaction_weight: f32,
    ) -> Result<(), Error> {
        ensure(
            self.valid() && reaction_weight.is_finite() && (0.0..=1.0).contains(&reaction_weight),
            "invalid physical blend request",
        )?;
        self.mode = mode;
        self.start = self.weight;
        self.duration = ticks;
        self.elapsed = 0;
        self.target = match mode {
            PhysicalMode::Animated | PhysicalMode::GettingUp => 0.0,
            PhysicalMode::HitReaction => reaction_weight,
            PhysicalMode::Ragdoll | PhysicalMode::Dead => 1.0,
        };
        if ticks == 0 {
            self.weight = self.target;
        }
        Ok(())
    }
    /// Completes one deterministic blend tick, independently of rendering rate.
    pub fn tick(&mut self) -> Result<(), Error> {
        ensure(self.valid(), "invalid physical blend state")?;
        self.elapsed = self.elapsed.saturating_add(1).min(self.duration);
        let t = if self.duration == 0 {
            1.0
        } else {
            self.elapsed as f32 / self.duration as f32
        };
        let smooth = t * t * (3.0 - 2.0 * t);
        self.weight = self.start + (self.target - self.start) * smooth;
        if self.mode == PhysicalMode::GettingUp && self.elapsed == self.duration {
            self.mode = PhysicalMode::Animated;
        }
        Ok(())
    }
    fn valid(&self) -> bool {
        [self.weight, self.start, self.target]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && self.elapsed <= self.duration
    }
}
/// One backend body's world pose and velocities, read without hidden allocations.
#[derive(Debug, Clone, Copy)]
pub struct BodyMotion {
    /// World-space body origin and rotation.
    pub pose: RootMotion,
    /// Linear velocity in m/s.
    pub linear: Vec3,
    /// Angular velocity in rad/s.
    pub angular: Vec3,
}
/// Narrow powered-body seam. Backend bindings/handles remain outside animation documents.
pub trait RagdollBackend {
    /// Reads a body by the stable profile order.
    fn body(&self, index: usize) -> Result<BodyMotion, Error>;
    /// Applies bounded world-space force and torque during the upcoming fixed tick.
    fn force(&mut self, index: usize, force: Vec3, torque: Vec3) -> Result<(), Error>;
    /// Applies a hit impulse at a world position.
    fn impulse(&mut self, index: usize, impulse: Vec3, point: Vec3) -> Result<(), Error>;
}
/// Immutable cooked body bindings plus actor-local model/body scratch.
#[derive(Debug)]
pub struct PhysicalAnimation {
    skeleton: u64,
    joints: Vec<usize>,
    offsets: Vec<RootMotion>,
    gains: Vec<MotorGains>,
    model: Vec<Mat4>,
    bodies: Vec<BodyMotion>,
}
impl PhysicalAnimation {
    /// Validates profile geometry, topology, axes and motor bounds before allocation-free use.
    pub fn cook(skeleton: &Skeleton, profile: &RagdollProfile) -> Result<Self, Error> {
        ensure(!profile.bones.is_empty(), "ragdoll profile is empty")?;
        let mut joints = Vec::new();
        let mut offsets = Vec::new();
        let mut gains = Vec::new();
        for (i, bone) in profile.bones.iter().enumerate() {
            let j = skeleton
                .data
                .joints
                .iter()
                .position(|j| j.id == bone.joint)
                .ok_or_else(|| Error("ragdoll joint is missing".into()))?;
            let offset = crate::Transform::from(bone.offset);
            let g = bone.motor;
            ensure(
                !joints.contains(&j)
                    && bone.parent.is_none_or(|p| (p as usize) < i)
                    && offset.valid()
                    && offset.scale == glam::Vec3A::ONE
                    && bone.radius.is_finite()
                    && bone.radius > 0.0
                    && bone.half_height.is_finite()
                    && bone.half_height >= 0.0
                    && bone.mass.is_finite()
                    && bone.mass > 0.0
                    && bone.limit.valid()
                    && [
                        g.position,
                        g.linear_damping,
                        g.rotation,
                        g.angular_damping,
                        g.max_force,
                        g.max_torque,
                    ]
                    .iter()
                    .all(|v| v.is_finite() && *v >= 0.0),
                "invalid ragdoll geometry, topology or gains",
            )?;
            joints.push(j);
            offsets.push(RootMotion::from_transform(offset));
            gains.push(g);
        }
        Ok(Self {
            skeleton: skeleton.signature(),
            bodies: vec![
                BodyMotion {
                    pose: RootMotion::default(),
                    linear: Vec3::ZERO,
                    angular: Vec3::ZERO
                };
                joints.len()
            ],
            joints,
            offsets,
            gains,
            model: vec![Mat4::IDENTITY; skeleton.len()],
        })
    }
    /// Submits bounded PD drives before physics stepping. Jolt constraints enforce joint limits.
    pub fn drive(
        &mut self,
        skeleton: &Skeleton,
        animation: &Pose,
        actor: RootMotion,
        state: &PhysicalState,
        backend: &mut impl RagdollBackend,
    ) -> Result<(), Error> {
        ensure(
            self.skeleton == skeleton.signature()
                && state.valid()
                && actor.translation.is_finite()
                && actor.rotation.is_finite()
                && actor.rotation.is_normalized(),
            "physical-animation rig/state mismatch",
        )?;
        model_matrices(skeleton, animation, &mut self.model)?;
        self.read(backend)?;
        let power = if state.mode == PhysicalMode::GettingUp {
            1.0
        } else {
            1.0 - state.weight
        };
        for i in 0..self.joints.len() {
            let (_, rotation, translation) =
                self.model[self.joints[i]].to_scale_rotation_translation();
            let desired = actor
                .compose(RootMotion {
                    translation,
                    rotation,
                })
                .compose(self.offsets[i]);
            let current = self.bodies[i];
            let gains = self.gains[i];
            let force = ((desired.translation - current.pose.translation) * gains.position
                - current.linear * gains.linear_damping)
                .clamp_length_max(gains.max_force)
                * power;
            let q = desired.rotation * current.pose.rotation.conjugate();
            let q = if q.w < 0.0 { -q } else { q };
            let torque = (q.to_scaled_axis() * gains.rotation
                - current.angular * gains.angular_damping)
                .clamp_length_max(gains.max_torque)
                * power;
            backend.force(i, force, torque)?;
        }
        Ok(())
    }
    /// Extracts post-physics poses and blends in model space before rebuilding locals.
    /// Unmapped helper bones retain animated locals. Actor root reconciliation is explicit.
    pub fn blend(
        &mut self,
        skeleton: &Skeleton,
        animation: &Pose,
        actor: RootMotion,
        state: &PhysicalState,
        backend: &impl RagdollBackend,
        out: &mut Pose,
    ) -> Result<(), Error> {
        ensure(
            self.skeleton == skeleton.signature()
                && state.valid()
                && out.len() == skeleton.len()
                && actor.translation.is_finite()
                && actor.rotation.is_finite()
                && actor.rotation.is_normalized(),
            "invalid physical pose blend",
        )?;
        self.read(backend)?;
        out.copy_from(animation)?;
        if state.weight == 0.0 {
            return Ok(());
        }
        model_matrices(skeleton, out, &mut self.model)?;
        // Iterate skeleton order, not body order, so partial profiles and helpers work.
        for j in 0..skeleton.len() {
            if let Some(i) = self.joints.iter().position(|joint| *joint == j) {
                let body = actor
                    .inverse()
                    .compose(self.bodies[i].pose)
                    .compose(self.offsets[i].inverse());
                let (_, animated_q, animated_t) = self.model[j].to_scale_rotation_translation();
                let desired_t = animated_t.lerp(body.translation, state.weight);
                let desired_q = nlerp(animated_q, body.rotation, state.weight);
                let parent = skeleton.parents[j].map_or(Mat4::IDENTITY, |p| self.model[p as usize]);
                let parent_q = parent.to_scale_rotation_translation().1;
                out.translations[j] = parent.inverse().transform_point3(desired_t).into();
                out.rotations[j] = (parent_q.conjugate() * desired_q).normalize();
            }
            let local = out
                .joint(j)
                .expect("joint is in the checked output pose")
                .matrix();
            self.model[j] = skeleton.parents[j].map_or(local, |p| self.model[p as usize] * local);
        }
        Ok(())
    }
    /// Computes a horizontal actor-root correction for a chosen get-up pose.
    /// The runtime applies this through its character controller before requesting recovery.
    pub fn get_up_root(&self, body: RootMotion, animated_pelvis: RootMotion) -> RootMotion {
        let forward = body.rotation * Vec3::Z;
        let yaw = forward.x.atan2(forward.z);
        let rotation = Quat::from_rotation_y(yaw);
        RootMotion {
            translation: body.translation - rotation * animated_pelvis.translation,
            rotation,
        }
    }
    fn read(&mut self, backend: &impl RagdollBackend) -> Result<(), Error> {
        for (i, body) in self.bodies.iter_mut().enumerate() {
            *body = backend.body(i)?;
            ensure(
                body.pose.translation.is_finite()
                    && body.pose.rotation.is_finite()
                    && body.pose.rotation.is_normalized()
                    && body.linear.is_finite()
                    && body.angular.is_finite(),
                "invalid backend ragdoll state",
            )?;
        }
        Ok(())
    }
}
