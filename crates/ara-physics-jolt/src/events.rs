use crate::JoltBackend;
use crate::convert::a;
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn update(&mut self, dt: f32, tick: u64, events: &mut PhysicsEvents) -> Result<()> {
        // SAFETY: exclusive live world; dt is validated by the C ABI.
        let status = self.run_jobs(|world| unsafe { sys::ara_world_step(world, dt) });
        if status != 0 {
            return Err(PhysicsError(format!(
                "Jolt update failed (flags {status:#x}): {}",
                Self::error()
            )));
        }
        if self.config.event_capacity == 0 {
            events.contacts.clear();
            events.triggers.clear();
            events.character_contacts.clear();
            return Ok(());
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
        raw.sort_unstable_by_key(|c| {
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
            .sort_unstable_by_key(|c| (c.character, c.other, c.subshape, c.phase));
        Ok(())
    }
}
