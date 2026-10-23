use super::*;
use anyhow::{Result, ensure};
use glam::Vec3;
/// Builds a texel-stable sun clipmap. World-page keys keep cache identities stable
/// as the camera crosses page boundaries; projection changes carry a revision.
pub fn directional(
    id: u64,
    direction: Vec3,
    eye: Vec3,
    level: u8,
    base_extent: f32,
    revision: u64,
) -> Result<Light> {
    ensure!(
        level < 24
            && base_extent.is_finite()
            && base_extent > 0.0
            && direction.is_finite()
            && direction.length_squared() > 1e-12
            && eye.is_finite(),
        "invalid directional clipmap"
    );
    let extent = base_extent * 2.0f32.powi(i32::from(level));
    let direction = direction.normalize();
    let up = if direction.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let rotation = Mat4::look_at_rh(Vec3::ZERO, direction, up);
    let mut center = rotation.transform_point3(eye);
    let step = extent * 2.0 / PAGE_SIDE as f32;
    center.x = (center.x / step).floor() * step;
    center.y = (center.y / step).floor() * step;
    let view = Mat4::from_translation(-center) * rotation;
    Ok(Light {
        id,
        projection: Projection::Directional(level),
        vp: Mat4::orthographic_rh(
            -extent,
            extent,
            -extent,
            extent,
            extent * 4.0,
            -extent * 4.0,
        ) * view,
        revision,
    })
}

/// Builds a reversed-Z spot projection with a finite light range.
pub fn spot(
    id: u64,
    position: Vec3,
    direction: Vec3,
    angle: f32,
    range: f32,
    revision: u64,
) -> Result<Light> {
    ensure!(
        position.is_finite()
            && direction.is_finite()
            && direction.length_squared() > 1e-12
            && angle.is_finite()
            && (0.01..3.13).contains(&angle)
            && range.is_finite()
            && range > 0.1,
        "invalid spot projection"
    );
    let d = direction.normalize();
    let up = if d.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Ok(Light {
        id,
        projection: Projection::Spot,
        vp: Mat4::perspective_rh(angle, 1.0, range, 0.05)
            * Mat4::look_at_rh(position, position + d, up),
        revision,
    })
}
/// Builds all six reversed-Z point-light cube projections in a stable face order.
pub fn point(id: u64, position: Vec3, range: f32, revision: u64) -> Result<[Light; 6]> {
    ensure!(
        position.is_finite() && range.is_finite() && range > 0.1,
        "invalid point projection"
    );
    let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, range, 0.05);
    let directions = [
        (Vec3::X, Vec3::Y),
        (-Vec3::X, Vec3::Y),
        (Vec3::Y, Vec3::Z),
        (-Vec3::Y, -Vec3::Z),
        (Vec3::Z, Vec3::Y),
        (-Vec3::Z, Vec3::Y),
    ];
    Ok(std::array::from_fn(|face| {
        let (direction, up) = directions[face];
        Light {
            id,
            projection: Projection::Cube(face as u8),
            vp: projection * Mat4::look_at_rh(position, position + direction, up),
            revision,
        }
    }))
}
