use crate::{BodyMeta, JoltBackend, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn add_bodies(
        &mut self,
        bodies: &[BodyDesc],
        out: &mut Vec<BodyHandle>,
    ) -> Result<()> {
        let mut raw = Vec::with_capacity(bodies.len());
        for b in bodies {
            self.require_shape(b.shape)?;
            raw.push(sys::AraBodyDesc {
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
                raw.as_ptr(),
                Self::count(raw.len())?,
                self.raw_ids.as_mut_ptr(),
            )
        })?;
        for (i, b) in bodies.iter().enumerate() {
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
                u64::from(raw[i].motion),
                u64::from(raw[i].layer),
                u64::from(raw[i].flags),
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
    pub(crate) fn update(&mut self, dt: f32, tick: u64, events: &mut PhysicsEvents) -> Result<()> {
        // SAFETY: exclusive live world; dt is validated by the C ABI.
        let status = unsafe { sys::ara_world_step(self.ptr(), dt) };
        if status != 0 {
            return Err(PhysicsError(format!(
                "Jolt update failed (flags {status:#x}): {}",
                Self::error()
            )));
        }
        // SAFETY: raw_contacts is allocated for the configured maximum event count;
        // workers have finished, so the queue is now single-threaded.
        let count = unsafe {
            sys::ara_contacts(
                self.ptr(),
                self.raw_contacts.as_mut_ptr(),
                Self::count(self.raw_contacts.len())?,
            )
        };
        if count == sys::INVALID {
            return Err(Self::error());
        }
        let raw = &mut self.raw_contacts[..count as usize];
        raw.sort_by_key(|c| {
            (
                (
                    c.kind_a,
                    c.body_a,
                    c.kind_b,
                    c.body_b,
                    c.subshape_a,
                    c.subshape_b,
                    c.phase,
                ),
                (
                    c.point.x.to_bits(),
                    c.point.y.to_bits(),
                    c.point.z.to_bits(),
                ),
                (
                    c.normal.x.to_bits(),
                    c.normal.y.to_bits(),
                    c.normal.z.to_bits(),
                ),
            )
        });
        events.contacts.clear();
        events.triggers.clear();
        events.character_contacts.clear();
        for c in raw {
            if c.kind_a == 1 {
                let key = (c.body_a, c.kind_b, c.body_b, c.subshape_b);
                if self
                    .character_contacts
                    .get(&key)
                    .is_some_and(|c| c.tick == tick)
                {
                    continue;
                }
                let phase = if self.character_contacts.contains_key(&key) {
                    ContactPhase::Persisted
                } else {
                    ContactPhase::Started
                };
                let contact = CharacterContactEvent {
                    tick,
                    character: CharacterHandle::from_raw(c.body_a),
                    user_data: self
                        .characters
                        .get(&CharacterHandle::from_raw(c.body_a))
                        .map_or(0, |m| m.user_data),
                    other_user_data: if c.kind_b == 1 {
                        self.characters
                            .get(&CharacterHandle::from_raw(c.body_b))
                            .map_or(0, |m| m.user_data)
                    } else {
                        self.bodies
                            .get(&BodyHandle::from_raw(c.body_b))
                            .map_or(0, |m| m.user_data)
                    },
                    other: if c.kind_b == 1 {
                        PhysicsActor::Character(CharacterHandle::from_raw(c.body_b))
                    } else {
                        PhysicsActor::Body(BodyHandle::from_raw(c.body_b))
                    },
                    subshape: c.subshape_b,
                    phase,
                    sensor: c.sensor != 0,
                    point: a(c.point),
                    normal: a(c.normal),
                };
                self.character_contacts.insert(key, contact);
                events.character_contacts.push(contact);
                continue;
            }
            let key = (c.body_a, c.body_b, c.subshape_a, c.subshape_b);
            let phase = match c.phase {
                0 => ContactPhase::Started,
                1 => ContactPhase::Persisted,
                _ => ContactPhase::Stopped,
            };
            let mut contact = ContactEvent {
                tick,
                body_a: BodyHandle::from_raw(c.body_a),
                body_b: BodyHandle::from_raw(c.body_b),
                user_data_a: self
                    .bodies
                    .get(&BodyHandle::from_raw(c.body_a))
                    .map_or(0, |m| m.user_data),
                user_data_b: self
                    .bodies
                    .get(&BodyHandle::from_raw(c.body_b))
                    .map_or(0, |m| m.user_data),
                subshape_a: c.subshape_a,
                subshape_b: c.subshape_b,
                phase,
                point: a(c.point),
                normal: a(c.normal),
            };
            let sensor = if c.phase == 2 {
                if let Some((sensor, previous)) = self.contacts.remove(&key) {
                    contact.point = previous.point;
                    contact.normal = previous.normal;
                    contact.user_data_a = previous.user_data_a;
                    contact.user_data_b = previous.user_data_b;
                    sensor
                } else {
                    false
                }
            } else {
                self.contacts.insert(key, (c.sensor != 0, contact));
                c.sensor != 0
            };
            if sensor {
                events.triggers.push(TriggerEvent { contact });
            } else {
                events.contacts.push(contact);
            }
        }
        self.character_contacts.retain(|_, contact| {
            if contact.tick != tick {
                let mut stopped = *contact;
                stopped.tick = tick;
                stopped.phase = ContactPhase::Stopped;
                events.character_contacts.push(stopped);
                false
            } else {
                true
            }
        });
        events
            .character_contacts
            .sort_by_key(|c| (c.character, c.other, c.subshape, c.phase));
        Ok(())
    }
}
