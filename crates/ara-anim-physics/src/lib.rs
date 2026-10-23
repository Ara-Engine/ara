//! Jolt contact and powered-ragdoll bindings through ara-anim's narrow traits.
#![forbid(unsafe_code)]
#![deny(missing_docs)]
use ara_anim::{
    Error, RootMotion, Skeleton,
    glam::{Mat4, Quat, Vec3},
    ik::{GroundHit, GroundQuery, LimitKind},
    physical::{BodyMotion, PhysicalAnimation, RagdollBackend, RagdollProfile},
};
use ara_physics::*;
use ara_physics_jolt::{JoltBackend, JoltWorld};

/// Short shared Jolt borrow for stable closest-hit foot probes.
pub struct JoltGround<'a> {
    /// Native world, borrowed only between physics updates.
    pub backend: &'a JoltBackend,
    /// Collision layers, sensor policy and actor exclusion.
    pub filter: QueryFilter,
}
impl GroundQuery for JoltGround<'_> {
    fn probe(
        &mut self,
        origin: Vec3,
        displacement: Vec3,
    ) -> std::result::Result<Option<GroundHit>, Error> {
        self.backend
            .ray_closest(origin.to_array(), displacement.to_array(), self.filter)
            .map(|hit| {
                hit.map(|h| GroundHit {
                    point: h.point.into(),
                    normal: h.normal.into(),
                })
            })
            .map_err(error)
    }
}
/// World-scoped bindings in immutable profile order. Recreate on lifecycle reconstruction.
#[derive(Debug)]
pub struct JoltRagdoll {
    bodies: Vec<BodyHandle>,
    joints: Vec<JointHandle>,
}
impl JoltRagdoll {
    /// Creates dynamic capsules and native hinge/six-DOF constraints outside the tick loop.
    /// `first_id` reserves consecutive nonzero application identities. `layer` is host policy.
    /// On failure, newly created constraints and bodies are removed; cached shapes remain.
    pub fn spawn(
        world: &mut JoltWorld,
        skeleton: &Skeleton,
        profile: &RagdollProfile,
        actor: RootMotion,
        first_id: u64,
        layer: u32,
    ) -> std::result::Result<Self, Error> {
        let _validated = PhysicalAnimation::cook(skeleton, profile)?;
        if first_id == 0
            || first_id.checked_add(profile.bones.len() as u64).is_none()
            || layer > 15
            || !actor.translation.is_finite()
            || !actor.rotation.is_finite()
            || !actor.rotation.is_normalized()
        {
            return Err(Error(
                "invalid ragdoll identity range, layer or actor transform".into(),
            ));
        }
        let mut rig = Self {
            bodies: Vec::with_capacity(profile.bones.len()),
            joints: Vec::with_capacity(profile.bones.len()),
        };
        let mut model = vec![Mat4::IDENTITY; skeleton.len()];
        ara_anim::pose::model_matrices(skeleton, skeleton.bind_pose(), &mut model)?;
        let result = rig.create(world, skeleton, profile, actor, first_id, layer, &model);
        if let Err(original) = result {
            rig.destroy(world)?;
            return Err(original);
        }
        // Reserve the backend's single-command staging buffer before the first tick.
        if let Some(body) = rig.bodies.first() {
            world.apply(&[BodyCommand::Wake(*body)]).map_err(error)?;
        }
        Ok(rig)
    }
    #[allow(clippy::too_many_arguments)]
    fn create(
        &mut self,
        world: &mut JoltWorld,
        skeleton: &Skeleton,
        profile: &RagdollProfile,
        actor: RootMotion,
        first_id: u64,
        layer: u32,
        model: &[Mat4],
    ) -> std::result::Result<(), Error> {
        for (i, bone) in profile.bones.iter().enumerate() {
            let j = skeleton
                .data()
                .joints
                .iter()
                .position(|j| j.id == bone.joint)
                .expect("profile was validated");
            let (_, rotation, translation) = model[j].to_scale_rotation_translation();
            let joint_pose = actor.compose(RootMotion {
                translation,
                rotation,
            });
            let offset = RootMotion::from_transform(bone.offset.into());
            let body_pose = joint_pose.compose(offset);
            let shape = world
                .backend_mut()
                .create_shape(if bone.half_height == 0.0 {
                    Shape::Sphere {
                        radius: bone.radius,
                    }
                } else {
                    Shape::Capsule {
                        radius: bone.radius,
                        half_height: bone.half_height,
                    }
                })
                .map_err(error)?;
            world
                .backend_mut()
                .create_bodies(
                    &[BodyDesc {
                        shape,
                        user_data: first_id + i as u64,
                        pose: physics_pose(body_pose),
                        body: RigidBody {
                            motion: MotionType::Dynamic,
                            mass: bone.mass,
                            allow_sleep: false,
                            ..Default::default()
                        },
                        collider: Collider {
                            layer,
                            ..Default::default()
                        },
                        velocity: Velocity::default(),
                    }],
                    &mut self.bodies,
                )
                .map_err(error)?;
            if let Some(parent) = bone.parent {
                let limit = bone.limit;
                let axis = joint_pose.rotation * Vec3::from(limit.axis);
                let normal = axis.any_orthonormal_vector();
                let swing = if limit.kind == LimitKind::Free {
                    std::f32::consts::PI
                } else {
                    limit.swing * std::f32::consts::FRAC_1_SQRT_2
                };
                let (min, max) = if limit.kind == LimitKind::Free {
                    (-std::f32::consts::PI, std::f32::consts::PI)
                } else {
                    (limit.min, limit.max)
                };
                let id = world
                    .backend_mut()
                    .create_joint(JointDesc {
                        body_a: Some(self.bodies[parent as usize]),
                        body_b: self.bodies[i],
                        joint: Joint {
                            kind: if limit.kind == LimitKind::Hinge {
                                JointKind::Hinge
                            } else {
                                JointKind::SixDof
                            },
                            anchor_a: joint_pose.translation.to_array(),
                            anchor_b: joint_pose.translation.to_array(),
                            axis: axis.to_array(),
                            normal: normal.to_array(),
                            rotation_min: [min, -swing, -swing],
                            rotation_max: [max, swing, swing],
                            ..Default::default()
                        },
                    })
                    .map_err(error)?;
                self.joints.push(id);
            }
        }
        Ok(())
    }
    /// Runtime bodies in the same stable order as the source profile.
    pub fn bodies(&self) -> &[BodyHandle] {
        &self.bodies
    }
    /// Borrows one world exclusively for a short drive/read/impulse phase.
    pub fn bind<'a>(&'a self, world: &'a mut JoltWorld) -> JoltBodies<'a> {
        JoltBodies { world, rig: self }
    }
    /// Tears down constraints first, then bodies, at an explicit lifecycle boundary.
    pub fn destroy(&mut self, world: &mut JoltWorld) -> std::result::Result<(), Error> {
        while let Some(joint) = self.joints.last().copied() {
            world.backend_mut().destroy_joint(joint).map_err(error)?;
            self.joints.pop();
        }
        while let Some(body) = self.bodies.last().copied() {
            world.backend_mut().destroy_body(body).map_err(error)?;
            self.bodies.pop();
        }
        Ok(())
    }
}
/// Exclusive per-tick view implementing the powered-body seam.
pub struct JoltBodies<'a> {
    world: &'a mut JoltWorld,
    rig: &'a JoltRagdoll,
}
impl RagdollBackend for JoltBodies<'_> {
    fn body(&self, index: usize) -> std::result::Result<BodyMotion, Error> {
        let body = *self
            .rig
            .bodies
            .get(index)
            .ok_or_else(|| Error("ragdoll body index is invalid".into()))?;
        let state = self.world.backend().body_state(body).map_err(error)?;
        Ok(BodyMotion {
            pose: RootMotion {
                translation: state.pose.position.into(),
                rotation: Quat::from_array(state.pose.rotation.0),
            },
            linear: state.velocity.linear.into(),
            angular: state.velocity.angular.into(),
        })
    }
    fn force(&mut self, index: usize, force: Vec3, torque: Vec3) -> std::result::Result<(), Error> {
        let body = *self
            .rig
            .bodies
            .get(index)
            .ok_or_else(|| Error("ragdoll body index is invalid".into()))?;
        self.world
            .apply(&[BodyCommand::Force {
                body,
                force: force.to_array(),
                torque: torque.to_array(),
            }])
            .map_err(error)
    }
    fn impulse(
        &mut self,
        index: usize,
        impulse: Vec3,
        point: Vec3,
    ) -> std::result::Result<(), Error> {
        let body = *self
            .rig
            .bodies
            .get(index)
            .ok_or_else(|| Error("ragdoll body index is invalid".into()))?;
        self.world
            .apply(&[BodyCommand::Impulse {
                body,
                impulse: impulse.to_array(),
                point: point.to_array(),
                angular: [0.0; 3],
            }])
            .map_err(error)
    }
}
fn physics_pose(pose: RootMotion) -> PhysicsPose {
    PhysicsPose {
        position: pose.translation.to_array(),
        rotation: Rotation(pose.rotation.to_array()),
    }
}
fn error(e: PhysicsError) -> Error {
    Error(e.0)
}
