//! Model-space analytic constraints. Callers choose their fixed evaluation order.

use crate::{Error, Pose, Skeleton, ensure, pose::nlerp};
use glam::{Mat4, Quat, Vec3};

/// Solves a two-segment chain geometrically using a target and a pole direction.
/// Returns desired model-space root and middle rotations. Segment lengths and
/// scales come from the supplied model matrices; unreachable targets are clamped.
pub fn two_bone(
    root: Mat4,
    middle: Mat4,
    tip: Mat4,
    target: Vec3,
    pole: Vec3,
    weight: f32,
) -> Result<(Quat, Quat), Error> {
    ensure(
        [root, middle, tip].iter().all(|m| m.is_finite())
            && target.is_finite()
            && pole.is_finite()
            && weight.is_finite()
            && (0.0..=1.0).contains(&weight),
        "invalid two-bone IK input",
    )?;
    let a = root.w_axis.truncate();
    let b = middle.w_axis.truncate();
    let c = tip.w_axis.truncate();
    let upper = b.distance(a);
    let lower = c.distance(b);
    ensure(
        upper > 1e-6 && lower > 1e-6,
        "IK segments must have nonzero length",
    )?;
    let to_target = target - a;
    let direction = to_target
        .try_normalize()
        .unwrap_or((c - a).try_normalize().unwrap_or(Vec3::X));
    let distance = to_target
        .length()
        .clamp((upper - lower).abs() + 1e-6, upper + lower - 1e-6);
    let projected = pole - direction * pole.dot(direction);
    let bend = projected
        .try_normalize()
        .unwrap_or_else(|| direction.any_orthonormal_vector());
    let along = (upper * upper + distance * distance - lower * lower) / (2.0 * distance);
    let height = (upper * upper - along * along).max(0.0).sqrt();
    let desired_b = a + direction * along + bend * height;
    let desired_c = a + direction * distance;
    let root_delta = Quat::from_rotation_arc((b - a).normalize(), (desired_b - a).normalize());
    let rotated_lower = root_delta * (c - b);
    let middle_delta = Quat::from_rotation_arc(
        rotated_lower.normalize(),
        (desired_c - desired_b).normalize(),
    );
    let (_, root_rotation, _) = root.to_scale_rotation_translation();
    let (_, middle_rotation, _) = middle.to_scale_rotation_translation();
    Ok((
        nlerp(root_rotation, root_delta * root_rotation, weight),
        nlerp(
            middle_rotation,
            middle_delta * root_delta * middle_rotation,
            weight,
        ),
    ))
}

/// Rotates a model-space orientation's local forward axis toward a target direction.
/// Angular change is clamped before applying the [0,1] constraint weight.
pub fn look_at(
    rotation: Quat,
    local_forward: Vec3,
    direction: Vec3,
    max_angle: f32,
    weight: f32,
) -> Result<Quat, Error> {
    ensure(
        rotation.is_finite()
            && rotation.is_normalized()
            && local_forward.is_finite()
            && direction.is_finite()
            && max_angle.is_finite()
            && max_angle >= 0.0
            && weight.is_finite()
            && (0.0..=1.0).contains(&weight),
        "invalid look-at input",
    )?;
    let forward = local_forward
        .try_normalize()
        .ok_or_else(|| Error("look-at forward must be nonzero".into()))?;
    let target = direction
        .try_normalize()
        .ok_or_else(|| Error("look-at direction must be nonzero".into()))?;
    let delta = Quat::from_rotation_arc(rotation * forward, target);
    let angle = 2.0 * delta.w.abs().clamp(0.0, 1.0).acos();
    let amount = if angle > 1e-6 {
        (max_angle / angle).min(1.0) * weight
    } else {
        weight
    };
    Ok((Quat::IDENTITY.slerp(delta, amount) * rotation).normalize())
}

/// Applies a solved model rotation as a local joint rotation, using current parents.
/// Rebuild model matrices between constraints that depend on one another.
pub fn apply_model_rotation(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &[Mat4],
    joint: usize,
    rotation: Quat,
) -> Result<(), Error> {
    ensure(
        joint < skeleton.len()
            && pose.len() == skeleton.len()
            && model.len() == skeleton.len()
            && rotation.is_finite()
            && rotation.is_normalized(),
        "invalid IK joint or model buffer",
    )?;
    let parent_rotation = skeleton.parents[joint].map_or(Quat::IDENTITY, |p| {
        model[p as usize].to_scale_rotation_translation().1
    });
    pose.rotations[joint] = (parent_rotation.conjugate() * rotation).normalize();
    Ok(())
}
