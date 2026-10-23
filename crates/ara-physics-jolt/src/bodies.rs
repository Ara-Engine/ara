use crate::{BodyMeta, JoltBackend, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn add_bodies(
        &mut self,
        bodies: &[BodyDesc],
        out: &mut Vec<BodyHandle>,
    ) -> Result<()> {
        self.raw_descriptions.clear();
        for b in bodies {
            self.require_shape(b.shape)?;
            self.raw_descriptions.push(sys::AraBodyDesc {
                shape: b.shape.raw(),
                motion: match b.body.motion {
                    MotionType::Static => 0,
                    MotionType::Kinematic => 1,
                    MotionType::Dynamic => 2,
                },
                layer: if b.collider.layer == 0 && b.body.motion != MotionType::Static {
                    1
                } else {
                    b.collider.layer
                },
                flags: u32::from(b.collider.sensor)
                    | (u32::from(b.body.allow_sleep) << 1)
                    | (u32::from(b.body.continuous) << 2),
                position: v(b.pose.position),
                rotation: q(b.pose.rotation),
                linear: v(b.velocity.linear),
                angular: v(b.velocity.angular),
                friction: b.collider.friction,
                restitution: b.collider.restitution,
                mass: b.body.mass,
                user_data: b.user_data,
            });
        }
        self.raw_ids.resize(bodies.len(), sys::INVALID);
        // SAFETY: descriptors/ID output hold exactly count entries; world is exclusive.
        Self::ok(unsafe {
            sys::ara_bodies_create(
                self.ptr(),
                self.raw_descriptions.as_ptr(),
                Self::count(self.raw_descriptions.len())?,
                self.raw_ids.as_mut_ptr(),
            )
        })?;
        for (i, b) in bodies.iter().enumerate() {
            let raw = self.raw_descriptions[i];
            let id = BodyHandle::from_raw(self.raw_ids[i]);
            out.push(id);
            self.bodies.insert(
                id,
                BodyMeta {
                    shape: b.shape,
                    user_data: b.user_data,
                },
            );
            self.changed(100);
            self.hash_words([
                u64::from(id.raw()),
                u64::from(b.shape.raw()),
                b.user_data,
                u64::from(raw.motion),
                u64::from(raw.layer),
                u64::from(raw.flags),
            ]);
            self.hash_floats(
                b.pose
                    .position
                    .into_iter()
                    .chain(b.pose.rotation.0)
                    .chain(b.velocity.linear)
                    .chain(b.velocity.angular)
                    .chain([b.body.mass, b.collider.friction, b.collider.restitution]),
            );
        }
        Ok(())
    }
    pub(crate) fn remove_body(&mut self, id: BodyHandle) -> Result<()> {
        self.require_body(id)?;
        // SAFETY: live world; native ownership/dependencies are checked before removal.
        Self::ok(unsafe { sys::ara_body_destroy(self.ptr(), id.raw()) })?;
        self.bodies.remove(&id);
        self.changed(101);
        self.hash_words([u64::from(id.raw())]);
        Ok(())
    }
    pub(crate) fn remove_bodies(&mut self, bodies: &[BodyHandle]) -> Result<()> {
        self.raw_ids.clear();
        self.raw_ids.extend(bodies.iter().map(|b| b.raw()));
        // SAFETY: IDs are borrowed for this synchronous call; native validates
        // every generation, dependency and duplicate before removing any body.
        Self::ok(unsafe {
            sys::ara_bodies_destroy(
                self.ptr(),
                self.raw_ids.as_ptr(),
                Self::count(bodies.len())?,
            )
        })?;
        for &body in bodies {
            self.bodies.remove(&body);
            self.changed(101);
            self.hash_words([u64::from(body.raw())]);
        }
        Ok(())
    }
    pub(crate) fn apply_bodies(&mut self, commands: &[BodyCommand], dt: f32) -> Result<()> {
        self.raw_commands.clear();
        for c in commands {
            let mut d = sys::AraBodyCommand {
                rotation: q(Rotation([0.0, 0.0, 0.0, 1.0])),
                ..Default::default()
            };
            match *c {
                BodyCommand::Teleport { body, pose } => {
                    d.body = body.raw();
                    d.kind = 0;
                    d.position = v(pose.position);
                    d.rotation = q(pose.rotation);
                }
                BodyCommand::KinematicTarget { body, pose } => {
                    d.body = body.raw();
                    d.kind = 1;
                    d.position = v(pose.position);
                    d.rotation = q(pose.rotation);
                }
                BodyCommand::Velocity { body, velocity } => {
                    d.body = body.raw();
                    d.kind = 2;
                    d.linear = v(velocity.linear);
                    d.angular = v(velocity.angular);
                }
                BodyCommand::Impulse {
                    body,
                    impulse,
                    point,
                    angular,
                } => {
                    d.body = body.raw();
                    d.kind = 3;
                    d.linear = v(impulse);
                    d.position = v(point);
                    d.angular = v(angular);
                }
                BodyCommand::Force {
                    body,
                    force,
                    torque,
                } => {
                    d.body = body.raw();
                    d.kind = 4;
                    d.linear = v(force);
                    d.angular = v(torque);
                }
                BodyCommand::Wake(body) => {
                    d.body = body.raw();
                    d.kind = 5;
                }
            }
            self.require_body(BodyHandle::from_raw(d.body))?;
            self.raw_commands.push(d);
        }
        // SAFETY: fully initialized command records, exclusive world; native validates
        // every command before applying any, including motion type and finite values.
        Self::ok(unsafe {
            sys::ara_bodies_apply(
                self.ptr(),
                self.raw_commands.as_ptr(),
                Self::count(self.raw_commands.len())?,
                dt,
            )
        })
    }
    pub(crate) fn extract_bodies(
        &mut self,
        bodies: &[BodyHandle],
        out: &mut Vec<BodyState>,
    ) -> Result<()> {
        self.raw_ids.clear();
        // The C batch validates bounds and generations in O(n) before writing any
        // output. A second BTreeMap membership pass here doubled extraction cost.
        self.raw_ids.extend(bodies.iter().map(|b| b.raw()));
        self.raw_bodies
            .resize(bodies.len(), sys::AraBodyState::default());
        // SAFETY: native ID/output buffers have count elements and exclusive access
        // precludes concurrent simulation while the no-lock extraction runs.
        Self::ok(unsafe {
            sys::ara_bodies_read(
                self.ptr(),
                self.raw_ids.as_ptr(),
                Self::count(bodies.len())?,
                self.raw_bodies.as_mut_ptr(),
            )
        })?;
        out.clear();
        out.extend(self.raw_bodies.iter().copied().map(state));
        Ok(())
    }
    pub(crate) fn extract_body(&self, body: BodyHandle) -> Result<BodyState> {
        let mut raw = sys::AraBodyState::default();
        let id = body.raw();
        // SAFETY: one valid body ID and one writable output, no concurrent mutation.
        Self::ok(unsafe { sys::ara_bodies_read(self.ptr(), &id, 1, &mut raw) })?;
        Ok(state(raw))
    }
    pub(crate) fn extract_columns(
        &mut self,
        bodies: &[BodyHandle],
        out: &mut BodyColumns,
    ) -> Result<()> {
        self.raw_ids.clear();
        self.raw_ids.extend(bodies.iter().map(|b| b.raw()));
        out.resize(bodies.len());
        // SAFETY: f32 arrays have the same size/alignment/order as the C vector
        // records (asserted below); all columns and IDs contain count elements.
        // Native validates every body before writing; mutation is excluded.
        Self::ok(unsafe {
            sys::ara_bodies_read_columns(
                self.ptr(),
                self.raw_ids.as_ptr(),
                Self::count(bodies.len())?,
                out.positions.as_mut_ptr().cast(),
                out.rotations.as_mut_ptr().cast(),
                out.linear.as_mut_ptr().cast(),
                out.angular.as_mut_ptr().cast(),
                out.active.as_mut_ptr(),
            )
        })
    }
}

const _: () = {
    assert!(size_of::<[f32; 3]>() == size_of::<sys::AraVec3>());
    assert!(align_of::<[f32; 3]>() == align_of::<sys::AraVec3>());
    assert!(size_of::<[f32; 4]>() == size_of::<sys::AraQuat>());
    assert!(align_of::<[f32; 4]>() == align_of::<sys::AraQuat>());
};
