//! Primitive dimensions are physics metres; visual scale is already a baked input.
use super::*;
use ara_physics::{Collider, ColliderKind};
use glam::Mat4;
const STEPS: usize = 32;

#[cfg(test)]
pub(super) fn collect(
    wires: &mut Vec<Segment>,
    counts: &mut Counts,
    view: &DocumentView,
    state: &EditorState,
) {
    let entities = view.entities();
    let transforms = crate::world::world_transforms(entities);
    collect_with(wires, counts, view, state, &transforms);
}
pub(super) fn collect_with(
    wires: &mut Vec<Segment>,
    counts: &mut Counts,
    view: &DocumentView,
    state: &EditorState,
    transforms: &std::collections::BTreeMap<String, Mat4>,
) {
    let entities = view.entities();
    for (id, entity) in entities {
        if state.hidden.contains(id) {
            continue;
        }
        let Some(value) = entity.record.get("Collider") else {
            continue;
        };
        let Ok(collider) = serde_json::from_value::<Collider>(value.clone()) else {
            counts.invalid_colliders += 1;
            continue;
        };
        let Some(matrix) = transforms.get(id) else {
            counts.invalid_colliders += 1;
            continue;
        };
        let Some(pose) = body_pose(*matrix) else {
            counts.invalid_colliders += 1;
            continue;
        };
        let before = wires.len();
        match shape(wires, &collider, pose) {
            Ok(true) => {
                counts.colliders += 1;
                counts.sensors += usize::from(collider.sensor);
            }
            Ok(false) => counts.unsupported_colliders += 1,
            Err(()) => {
                wires.truncate(before);
                counts.invalid_colliders += 1;
            }
        }
    }
}
fn body_pose(matrix: Mat4) -> Option<Mat4> {
    if !matrix.is_finite()
        || [matrix.x_axis, matrix.y_axis, matrix.z_axis]
            .iter()
            .any(|axis| {
                let length = axis.truncate().length_squared();
                !length.is_finite() || length == 0.
            })
    {
        return None;
    }
    let determinant = matrix.determinant();
    if !determinant.is_finite() || determinant == 0. {
        return None;
    }
    // Same decomposition as runtime Pose::from_matrix and Pose::to_physics:
    // discard shear/visual scale; Collider dimensions/centre are physics metres.
    let (_, rotation, translation) = matrix.to_scale_rotation_translation();
    if !rotation.is_finite() || rotation.length_squared() == 0. {
        return None;
    }
    let rotation = rotation.normalize();
    (rotation.is_finite() && translation.is_finite())
        .then(|| Mat4::from_rotation_translation(rotation, translation))
}
fn shape(wires: &mut Vec<Segment>, collider: &Collider, pose: Mat4) -> Result<bool, ()> {
    if matches!(
        collider.kind,
        ColliderKind::Cooked | ColliderKind::ModelConvex | ColliderKind::ModelMesh
    ) {
        return Ok(false);
    }
    let center = Vec3::from_array(collider.center);
    if !center.is_finite() {
        return Err(());
    }
    let world = pose * Mat4::from_translation(center);
    let tint = if collider.sensor {
        Tint::Sensor
    } else {
        Tint::Collider
    };
    if collider.kind == ColliderKind::Box {
        let half = Vec3::from_array(collider.half_extents);
        if !half.is_finite() || half.min_element() <= 0. {
            return Err(());
        }
        bounds::box_lines(wires, -half, half, world, tint);
        return Ok(true);
    }
    let radius = collider.radius;
    if !radius.is_finite() || radius <= 0. {
        return Err(());
    }
    if collider.kind == ColliderKind::Sphere {
        sphere(wires, world, radius, tint);
        return Ok(true);
    }
    let height = collider.half_height;
    if !height.is_finite() || height < 0. {
        return Err(());
    }
    if collider.kind == ColliderKind::Capsule && height == 0. {
        sphere(wires, world, radius, tint);
        return Ok(true);
    }
    for sign in [-1., 1.] {
        arc(
            wires,
            world,
            Vec3::Y * height * sign,
            Vec3::X,
            Vec3::Z,
            radius,
            0.,
            std::f32::consts::TAU,
            tint,
        );
    }
    for axis in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        line(
            wires,
            world,
            axis * radius - Vec3::Y * height,
            axis * radius + Vec3::Y * height,
            tint,
        );
    }
    if collider.kind == ColliderKind::Capsule {
        for axis in [Vec3::X, Vec3::Z] {
            arc(
                wires,
                world,
                Vec3::Y * height,
                axis,
                Vec3::Y,
                radius,
                0.,
                std::f32::consts::PI,
                tint,
            );
            arc(
                wires,
                world,
                Vec3::NEG_Y * height,
                axis,
                Vec3::Y,
                radius,
                std::f32::consts::PI,
                std::f32::consts::TAU,
                tint,
            );
        }
    }
    Ok(true)
}
fn sphere(wires: &mut Vec<Segment>, world: Mat4, radius: f32, tint: Tint) {
    for (u, v) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
        arc(
            wires,
            world,
            Vec3::ZERO,
            u,
            v,
            radius,
            0.,
            std::f32::consts::TAU,
            tint,
        );
    }
}
#[allow(
    clippy::too_many_arguments,
    reason = "a circle arc has independent basis, centre and angular range"
)]
fn arc(
    wires: &mut Vec<Segment>,
    world: Mat4,
    center: Vec3,
    u: Vec3,
    v: Vec3,
    radius: f32,
    start: f32,
    end: f32,
    tint: Tint,
) {
    let point = |index| {
        let angle = start + (end - start) * index as f32 / STEPS as f32;
        center + radius * (u * angle.cos() + v * angle.sin())
    };
    for index in 0..STEPS {
        line(wires, world, point(index), point(index + 1), tint);
    }
}
fn line(wires: &mut Vec<Segment>, world: Mat4, from: Vec3, to: Vec3, tint: Tint) {
    wires.push(Segment {
        from: world.transform_point3(from),
        to: world.transform_point3(to),
        tint,
    });
}
