//! Explicit field conversions keep Rust independent of native padding/alignment.
use ara_jolt_sys as sys;
use ara_physics::*;

pub(crate) fn v(a: [f32; 3]) -> sys::AraVec3 {
    sys::AraVec3 {
        x: a[0],
        y: a[1],
        z: a[2],
    }
}
pub(crate) fn a(v: sys::AraVec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}
pub(crate) fn q(a: Rotation) -> sys::AraQuat {
    let [x, y, z, w] = a.0;
    sys::AraQuat { x, y, z, w }
}

pub(crate) fn state(s: sys::AraBodyState) -> BodyState {
    BodyState {
        body: BodyHandle::from_raw(s.body),
        active: s.active != 0,
        pose: PhysicsPose {
            position: a(s.position),
            rotation: Rotation([s.rotation.x, s.rotation.y, s.rotation.z, s.rotation.w]),
        },
        velocity: Velocity {
            linear: a(s.linear),
            angular: a(s.angular),
        },
    }
}
pub(crate) fn hit(h: sys::AraHit) -> QueryHit {
    QueryHit {
        body: BodyHandle::from_raw(h.body),
        subshape: h.subshape,
        user_data: h.user_data,
        fraction: h.fraction,
        penetration: h.depth,
        point: a(h.point),
        normal: a(h.normal),
    }
}
