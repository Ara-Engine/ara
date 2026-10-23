use crate::{CharacterMeta, JoltBackend, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn add_character(&mut self, desc: CharacterDesc) -> Result<CharacterHandle> {
        let c = desc.controller;
        let values = [
            c.radius,
            c.height,
            c.crouch_height,
            c.max_slope_deg,
            c.max_strength,
            c.mass,
            c.padding,
            c.step_height,
            c.stick_down,
        ];
        if !values.iter().all(|v| v.is_finite()) {
            return Err(PhysicsError("character settings must be finite".into()));
        }
        let d = sys::AraCharacterDesc {
            radius: c.radius,
            height: c.height,
            crouch_height: c.crouch_height,
            max_slope_deg: c.max_slope_deg,
            max_strength: c.max_strength,
            mass: c.mass,
            padding: c.padding,
            step_up: c.step_height,
            stick_down: c.stick_down,
            position: v(desc.position),
            layer: c.layer,
        };
        // SAFETY: initialized settings, exclusively owned world; dimensions checked natively.
        let id = unsafe { sys::ara_character_create(self.ptr(), &d) };
        if id == sys::INVALID {
            return Err(Self::error());
        }
        let handle = CharacterHandle::from_raw(id);
        self.characters.insert(
            handle,
            CharacterMeta {
                controller: c,
                user_data: desc.user_data,
            },
        );
        self.changed(200);
        self.hash_words([u64::from(id), u64::from(c.layer), desc.user_data]);
        self.hash_floats(values.into_iter().chain(desc.position));
        Ok(handle)
    }
    pub(crate) fn remove_character(&mut self, id: CharacterHandle) -> Result<()> {
        // SAFETY: exclusive world; the shim validates character existence before use.
        Self::ok(unsafe { sys::ara_character_destroy(self.ptr(), id.raw()) })?;
        self.characters.remove(&id);
        self.changed(201);
        self.hash_words([u64::from(id.raw())]);
        Ok(())
    }
    pub(crate) fn move_character(
        &mut self,
        id: CharacterHandle,
        input: CharacterInput,
        dt: f32,
    ) -> Result<()> {
        let d = sys::AraCharacterInput {
            character: id.raw(),
            desired: v(input.desired_velocity),
            jump_speed: input.jump_speed,
            crouch: u32::from(input.crouch),
        };
        // SAFETY: initialized typed input; exclusive world, with no Rust callbacks.
        Self::ok(unsafe { sys::ara_character_step(self.ptr(), &d, dt) })
    }
    pub(crate) fn extract_character(&self, id: CharacterHandle) -> Result<CharacterState> {
        let mut s = sys::AraCharacterState::default();
        // SAFETY: one initialized output record; shim checks the handle, no mutation.
        Self::ok(unsafe { sys::ara_character_read(self.ptr(), id.raw(), &mut s) })?;
        Ok(CharacterState {
            position: a(s.position),
            velocity: a(s.linear),
            ground_velocity: a(s.ground_velocity),
            ground_normal: a(s.ground_normal),
            ground_body: (s.ground_body != sys::INVALID)
                .then(|| BodyHandle::from_raw(s.ground_body)),
            grounded: s.ground_state == 0,
            crouched: s.crouched != 0,
        })
    }
}
