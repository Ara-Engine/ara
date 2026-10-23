//! Engine transform conventions, shared by simulation sync and presentation.
//!
//! - Y-up, right-handed, metres.
//! - `Transform.rotation` holds intrinsic XYZ Euler angles in degrees
//!   (`glam::EulerRot::XYZ`), relative to the parent.
//! - Cameras and lights look along their local -Z axis. Characters face +Z at yaw 0.
use ara_components::Transform;
use ara_physics::PhysicsPose;
use glam::{EulerRot, Mat4, Quat, Vec3};

/// Translation, rotation and scale. World poses are decomposed world matrices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Position in metres.
    pub translation: Vec3,
    /// Unit rotation.
    pub rotation: Quat,
    /// Per-axis scale.
    pub scale: Vec3,
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Pose {
    /// No translation, rotation or scale.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// The local pose an authored transform describes.
    pub fn from_transform(t: &Transform) -> Self {
        let [x, y, z] = t.rotation.map(f32::to_radians);
        Self {
            translation: Vec3::from_array(t.position),
            rotation: Quat::from_euler(EulerRot::XYZ, x, y, z),
            scale: Vec3::from_array(t.scale),
        }
    }

    /// The authored form of this pose (Euler degrees).
    pub fn to_transform(self) -> Transform {
        let (x, y, z) = self.rotation.to_euler(EulerRot::XYZ);
        Transform {
            position: self.translation.to_array(),
            rotation: [x, y, z].map(f32::to_degrees),
            scale: self.scale.to_array(),
        }
    }

    /// Column-major affine matrix.
    pub fn matrix(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Decomposes an affine matrix (shear is discarded).
    pub fn from_matrix(m: Mat4) -> Self {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        Self {
            translation,
            rotation: rotation.normalize(),
            scale,
        }
    }

    /// Presentation interpolation: lerped translation/scale, shortest-arc slerp.
    pub fn lerp(self, to: Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(to.translation, t),
            rotation: self.rotation.slerp(to.rotation, t),
            scale: self.scale.lerp(to.scale, t),
        }
    }

    /// The physics pose of this world pose (scale is baked into colliders instead).
    pub fn to_physics(self) -> PhysicsPose {
        PhysicsPose {
            position: self.translation.to_array(),
            rotation: ara_physics::Rotation(self.rotation.to_array()),
        }
    }

    /// A world pose from a physics pose, keeping `scale`.
    pub fn from_physics(pose: &PhysicsPose, scale: Vec3) -> Self {
        Self {
            translation: Vec3::from_array(pose.position),
            rotation: Quat::from_array(pose.rotation.0).normalize(),
            scale,
        }
    }
}

/// Unit rotation turning +Z toward `yaw` degrees about +Y.
pub fn yaw_rotation(yaw: f32) -> Quat {
    Quat::from_rotation_y(yaw.to_radians())
}

/// Moves `current` toward `target` (degrees) by at most `step`, along the shorter arc.
pub fn approach_angle(current: f32, target: f32, step: f32) -> f32 {
    let delta = (target - current + 540.0).rem_euclid(360.0) - 180.0;
    let moved = current + delta.clamp(-step, step);
    (moved + 180.0).rem_euclid(360.0) - 180.0
}

/// A stable 64-bit identity for a document address (FNV-1a), used for render IDs,
/// capture marks and navigation agent IDs. Never persisted as a reference.
pub fn stable_id(address: &str) -> u64 {
    address.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn transforms_round_trip_and_angles_take_the_short_arc() {
        let t = Transform {
            position: [1.0, 2.0, 3.0],
            rotation: [10.0, 35.0, -20.0],
            scale: [2.0, 1.0, 0.5],
        };
        let back = Pose::from_transform(&t).to_transform();
        for i in 0..3 {
            assert!((back.rotation[i] - t.rotation[i]).abs() < 1e-3);
        }
        let pose = Pose::from_transform(&t);
        let m = Pose::from_matrix(pose.matrix());
        assert!(m.translation.abs_diff_eq(pose.translation, 1e-5));
        assert_eq!(approach_angle(170.0, -170.0, 5.0), 175.0);
        assert_eq!(approach_angle(0.0, 90.0, 400.0), 90.0);
        assert_ne!(stable_id("e_0001"), stable_id("e_0002"));
        // Yaw 90 turns +Z toward +X.
        assert!((yaw_rotation(90.0) * Vec3::Z).abs_diff_eq(Vec3::X, 1e-6));
    }
}
