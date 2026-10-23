use crate::{BodyMeta, JoltBackend, VehicleMeta, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn add_vehicle(&mut self, desc: VehicleDesc) -> Result<VehicleHandle> {
        let c = desc.vehicle;
        let values = [
            c.mass,
            c.wheel_radius,
            c.wheel_width,
            c.suspension_min,
            c.suspension_max,
            c.suspension_frequency,
            c.suspension_damping,
            c.max_steer_deg,
            c.engine_max_torque,
        ];
        if !values.iter().all(|v| v.is_finite()) {
            return Err(PhysicsError("vehicle settings must be finite".into()));
        }
        let shape = self.cook(Shape::Box {
            half_extents: c.half_extents,
        })?;
        let d = sys::AraVehicleDesc {
            position: v(desc.pose.position),
            rotation: q(desc.pose.rotation),
            half_extents: v(c.half_extents),
            mass: c.mass,
            wheel_radius: c.wheel_radius,
            wheel_width: c.wheel_width,
            wheel_offsets: desc.wheel_offsets.map(v),
            suspension_min: c.suspension_min,
            suspension_max: c.suspension_max,
            suspension_frequency: c.suspension_frequency,
            suspension_damping: c.suspension_damping,
            max_steer_deg: c.max_steer_deg,
            engine_max_torque: c.engine_max_torque,
            rear_wheel_drive: u32::from(c.rear_wheel_drive),
            layer: c.layer,
            user_data: desc.user_data,
        };
        // SAFETY: initialized descriptor and exclusive world. Native validates dimensions.
        let id = unsafe { sys::ara_vehicle_create(self.ptr(), &d) };
        if id == sys::INVALID {
            return Err(Self::error());
        }
        let handle = VehicleHandle::from_raw(id);
        let state = self.extract_vehicle(handle)?;
        self.vehicles.insert(handle, VehicleMeta { desc });
        self.bodies.insert(
            state.chassis,
            BodyMeta {
                shape,
                user_data: desc.user_data,
            },
        );
        self.changed(300);
        self.hash_words([
            u64::from(id),
            desc.user_data,
            u64::from(c.layer),
            u64::from(c.rear_wheel_drive),
        ]);
        self.hash_floats(
            values
                .into_iter()
                .chain(c.half_extents)
                .chain(desc.pose.position)
                .chain(desc.pose.rotation.0),
        );
        for p in desc.wheel_offsets {
            self.hash_floats(p);
        }
        Ok(handle)
    }
    pub(crate) fn remove_vehicle(&mut self, id: VehicleHandle) -> Result<()> {
        let state = self.extract_vehicle(id)?;
        // SAFETY: exclusive live world; native refuses a chassis with attached joints.
        Self::ok(unsafe { sys::ara_vehicle_destroy(self.ptr(), id.raw()) })?;
        self.bodies.remove(&state.chassis);
        self.vehicles.remove(&id);
        self.changed(301);
        self.hash_words([u64::from(id.raw())]);
        Ok(())
    }
    pub(crate) fn drive_vehicle(&mut self, id: VehicleHandle, input: VehicleInput) -> Result<()> {
        let d = sys::AraVehicleInput {
            vehicle: id.raw(),
            throttle: input.throttle,
            steer: input.steer,
            brake: input.brake,
            hand_brake: input.hand_brake,
        };
        // SAFETY: initialized input; native validates handle/ranges before mutation.
        Self::ok(unsafe { sys::ara_vehicle_input(self.ptr(), &d) })
    }
    pub(crate) fn extract_vehicle(&self, id: VehicleHandle) -> Result<VehicleState> {
        let mut s = sys::AraVehicleState::default();
        // SAFETY: live shared world and one writable output; native validates handle.
        Self::ok(unsafe { sys::ara_vehicle_read(self.ptr(), id.raw(), &mut s) })?;
        Ok(VehicleState {
            chassis: BodyHandle::from_raw(s.chassis),
            wheels_in_contact: s.wheels_in_contact,
            engine_rpm: s.engine_rpm,
            gear: s.gear,
            wheel_positions: s.wheel_positions.map(a),
        })
    }
}
