//! Follow-camera placement, shared by Play ([`crate::control::place_cameras`]) and the
//! editor's opening view so both frame a target the same way: the rig's orbit, and a
//! boom that stops in front of walls instead of looking through them.
use crate::components::FollowCamera;
use ara_physics::{Collider, ColliderKind, MotionType, RigidBody};
use glam::{Quat, Vec3};

/// How far in front of a wall a shortened boom stops, in metres.
pub const BOOM_MARGIN: f32 = 0.25;
/// The shortest boom a wall leaves a rig, in metres (never longer than the rig's own).
pub const MIN_BOOM: f32 = 0.3;

/// A solid box a follow camera's boom stops in front of. As in physics, the body's
/// world scale is not applied to the collider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wall {
    /// Box centre in world space.
    pub center: Vec3,
    /// Box orientation in world space.
    pub rotation: Quat,
    /// Half extents in metres.
    pub half_extents: Vec3,
}

impl Wall {
    /// Whether a body stops booms: static, solid (not a sensor) box colliders only.
    /// Moving bodies and characters don't, so the camera never jumps in for a crate.
    pub fn solid(collider: &Collider, body: &RigidBody) -> bool {
        collider.kind == ColliderKind::Box && !collider.sensor && body.motion == MotionType::Static
    }

    /// The wall `collider` makes on a body at world `translation` and `rotation`.
    pub fn new(collider: &Collider, translation: Vec3, rotation: Quat) -> Self {
        Self {
            center: translation + rotation * Vec3::from_array(collider.center),
            rotation,
            half_extents: Vec3::from_array(collider.half_extents),
        }
    }

    /// How far along unit `direction` from `origin` the ray enters the box, within
    /// `max`; `None` when it misses, the box is behind or beyond `max`, or `origin` is
    /// inside it.
    pub fn entry(&self, origin: Vec3, direction: Vec3, max: f32) -> Option<f32> {
        // Most walls are nowhere near the boom: reject on the bounding sphere first.
        let to = self.center - origin;
        let along = to.dot(direction);
        let radius = self.half_extents.length();
        if along + radius < 0.
            || along - radius > max
            || (to - direction * along).length_squared() > radius * radius
        {
            return None;
        }
        let inverse = self.rotation.inverse();
        let from = inverse * (origin - self.center);
        let along = inverse * direction;
        let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
        for axis in 0..3 {
            let half = self.half_extents[axis];
            if along[axis].abs() < 1e-8 {
                if from[axis].abs() > half {
                    return None;
                }
                continue;
            }
            let a = (-half - from[axis]) / along[axis];
            let b = (half - from[axis]) / along[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
        (near > 0. && near <= far && near <= max).then_some(near)
    }
}

/// Where a follow rig puts its camera: at [`FollowView::eye`], looking along `forward`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowView {
    /// The point the camera looks at: the target raised by the rig's height.
    pub aim: Vec3,
    /// Unit view direction from the rig's yaw and pitch (yaw 0 looks along +Z).
    pub forward: Vec3,
    /// Boom length: the rig's distance, or shorter in front of a wall.
    pub distance: f32,
}

impl FollowView {
    /// Where `rig` looks from for a target whose origin is at `target`. The boom stops
    /// [`BOOM_MARGIN`] in front of the nearest of `walls` behind the aim point (never
    /// under [`MIN_BOOM`]), so a target by a wall is seen from inside the room.
    pub fn new(rig: &FollowCamera, target: Vec3, walls: impl IntoIterator<Item = Wall>) -> Self {
        let (yaw, pitch) = (rig.yaw.to_radians(), rig.pitch.to_radians());
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let aim = target + Vec3::Y * rig.height;
        // A wall up to the margin past the rig's distance still pulls the camera in.
        let reach = walls
            .into_iter()
            .filter_map(|wall| wall.entry(aim, -forward, rig.distance + BOOM_MARGIN))
            .fold(f32::INFINITY, f32::min);
        Self {
            aim,
            forward,
            distance: rig.distance.min((reach - BOOM_MARGIN).max(MIN_BOOM)),
        }
    }

    /// The camera position.
    pub fn eye(&self) -> Vec3 {
        self.aim - self.forward * self.distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rig 3.6 m behind a target at the origin, looking along +Z.
    fn rig() -> FollowCamera {
        FollowCamera {
            distance: 3.6,
            height: 1.5,
            pitch: 0.,
            ..Default::default()
        }
    }

    /// A 0.2 m thick wall across the boom whose near face is `at` metres behind the aim.
    fn wall_behind(at: f32) -> Wall {
        Wall::new(
            &Collider {
                half_extents: [5., 3., 0.1],
                ..Default::default()
            },
            Vec3::new(0., 1.5, -at - 0.1),
            Quat::IDENTITY,
        )
    }

    #[test]
    fn a_wall_behind_the_target_shortens_the_boom() {
        let view = FollowView::new(&rig(), Vec3::ZERO, [wall_behind(1.9)]);
        assert!((view.distance - 1.65).abs() < 1e-5, "{view:?}");
        assert!((view.eye() - Vec3::new(0., 1.5, -1.65)).length() < 1e-5);
        // Too close: the minimum boom (the near plane may clip, the view still points
        // the right way). Out of reach: the rig's own distance.
        assert_eq!(
            FollowView::new(&rig(), Vec3::ZERO, [wall_behind(0.2)]).distance,
            MIN_BOOM
        );
        assert_eq!(
            FollowView::new(&rig(), Vec3::ZERO, [wall_behind(9.)]).distance,
            3.6
        );
        // Within the margin past the rig's distance, the camera still keeps clear.
        assert!(
            (FollowView::new(&rig(), Vec3::ZERO, [wall_behind(3.7)]).distance - 3.45).abs() < 1e-5
        );
    }

    #[test]
    fn walls_are_rotated_static_solid_boxes() {
        // A thin wall turned 90° about Y, off its body's origin, spans the boom only
        // once rotated.
        let turned = Wall::new(
            &Collider {
                half_extents: [0.1, 3., 5.],
                center: [1., 0., 0.],
                ..Default::default()
            },
            Vec3::new(0., 1.5, -1.),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let view = FollowView::new(&rig(), Vec3::ZERO, [turned]);
        assert!((view.distance - 1.65).abs() < 1e-5, "{view:?}");
        // A box around the aim point (the player's own trigger, a room volume) is not a wall.
        let around = Wall::new(&Collider::default(), Vec3::Y * 1.5, Quat::IDENTITY);
        assert_eq!(around.entry(Vec3::Y * 1.5, Vec3::NEG_Z, 10.), None);

        let body = |motion| RigidBody {
            motion,
            ..Default::default()
        };
        let collider = Collider::default();
        assert!(Wall::solid(&collider, &body(MotionType::Static)));
        assert!(!Wall::solid(&collider, &body(MotionType::Dynamic)));
        assert!(!Wall::solid(&collider, &body(MotionType::Kinematic)));
        let sensor = Collider {
            sensor: true,
            ..Default::default()
        };
        assert!(!Wall::solid(&sensor, &body(MotionType::Static)));
        let sphere = Collider {
            kind: ColliderKind::Sphere,
            ..Default::default()
        };
        assert!(!Wall::solid(&sphere, &body(MotionType::Static)));
    }
}
