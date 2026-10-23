use crate::{JoltBackend, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn add_joint(&mut self, desc: JointDesc) -> Result<JointHandle> {
        self.require_body(desc.body_b)?;
        if let Some(a) = desc.body_a {
            self.require_body(a)?;
        }
        let j = desc.joint;
        let kind = match j.kind {
            JointKind::Fixed => 0,
            JointKind::Hinge => 1,
            JointKind::Slider => 2,
            JointKind::Distance => 3,
            JointKind::Point => 4,
            JointKind::SixDof => 5,
        };
        let mut lower = [0.0; 6];
        lower[..3].copy_from_slice(&j.translation_min);
        lower[3..].copy_from_slice(&j.rotation_min);
        let mut upper = [0.0; 6];
        upper[..3].copy_from_slice(&j.translation_max);
        upper[3..].copy_from_slice(&j.rotation_max);
        if j.kind == JointKind::SixDof
            && j.rotation_min
                .into_iter()
                .chain(j.rotation_max)
                .any(|v| v.abs() > std::f32::consts::PI)
        {
            return Err(PhysicsError(
                "six-DOF angular limits must be in -pi..pi".into(),
            ));
        }
        let d = sys::AraJointDesc {
            kind,
            body_a: desc.body_a.map_or(sys::INVALID, BodyHandle::raw),
            body_b: desc.body_b.raw(),
            anchor_a: v(j.anchor_a),
            anchor_b: v(j.anchor_b),
            axis: v(j.axis),
            normal: v(j.normal),
            lower,
            upper,
        };
        // SAFETY: initialized descriptor, checked body handles, exclusive native world.
        let id = unsafe { sys::ara_joint_create(self.ptr(), &d) };
        if id == sys::INVALID {
            return Err(Self::error());
        }
        let handle = JointHandle::from_raw(id);
        self.joints.insert(handle, desc);
        self.changed(400);
        self.hash_words([
            u64::from(id),
            u64::from(kind),
            u64::from(d.body_a),
            u64::from(d.body_b),
        ]);
        self.hash_floats(
            j.anchor_a
                .into_iter()
                .chain(j.anchor_b)
                .chain(j.axis)
                .chain(j.normal)
                .chain(lower)
                .chain(upper),
        );
        Ok(handle)
    }
    pub(crate) fn remove_joint(&mut self, id: JointHandle) -> Result<()> {
        // SAFETY: exclusive native world; the shim checks existence before removal.
        Self::ok(unsafe { sys::ara_joint_destroy(self.ptr(), id.raw()) })?;
        self.joints.remove(&id);
        self.changed(401);
        self.hash_words([u64::from(id.raw())]);
        Ok(())
    }
}
