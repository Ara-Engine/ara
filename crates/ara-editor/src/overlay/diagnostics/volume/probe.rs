//! Mirrors runtime-render/captures::authored over the runtime's decomposed world pose.
use super::*;
use glam::{Quat, Vec3};

struct Geometry {
    position: Vec3,
    rotation: Quat,
    influence: Vec3,
    projection_centre: Vec3,
    projection: Vec3,
    capture: Vec3,
}
impl Geometry {
    fn read(record: &ara_doc::Record, world: Mat4) -> Option<Self> {
        let component: ara_components::render::ReflectionProbe =
            serde::Deserialize::deserialize(record.get("ReflectionProbe")?).ok()?;
        if !component.enabled || component.invalid().is_some() {
            return None;
        }
        let determinant = world.determinant();
        if !world.is_finite()
            || !determinant.is_finite()
            || determinant == 0.
            || [world.x_axis, world.y_axis, world.z_axis]
                .iter()
                .any(|axis| {
                    let squared = axis.truncate().length_squared();
                    !squared.is_finite() || squared == 0.
                })
        {
            return None;
        }
        let (_, rotation, _) = world.to_scale_rotation_translation();
        if !rotation.is_finite()
            || !rotation.length_squared().is_finite()
            || rotation.length_squared() < 1e-12
        {
            return None;
        }
        // Extraction uses Pose::from_matrix, discarding shear and normalizing its
        // quaternion. Do not transform boxes/offsets with the original sheared matrix.
        let pose = ara_runtime::math::Pose::from_matrix(glam34::Mat4::from_cols_array(
            &world.to_cols_array(),
        ));
        let position = Vec3::from_array(pose.translation.to_array());
        let rotation = Quat::from_array(pose.rotation.to_array()).normalize();
        let scale = Vec3::from_array(pose.scale.to_array())
            .abs()
            .max(Vec3::splat(1e-4));
        let influence = (Vec3::from_array(component.bounds) * 0.5 * scale).max(Vec3::splat(0.005));
        let projection = Vec3::from_array(component.projection_bounds) * 0.5 * scale;
        let projection = if projection.min_element() > 0. {
            projection
        } else {
            influence
        };
        let projection_centre =
            position + rotation * (Vec3::from_array(component.projection_offset) * scale);
        let capture = position + rotation * (Vec3::from_array(component.capture_offset) * scale);
        (position.is_finite()
            && rotation.is_finite()
            && influence.is_finite()
            && projection.is_finite()
            && projection_centre.is_finite()
            && capture.is_finite())
        .then_some(Self {
            position,
            rotation,
            influence,
            projection_centre,
            projection,
            capture,
        })
    }
}

pub(super) fn collect(wires: &mut Vec<Segment>, record: &ara_doc::Record, world: Mat4) {
    let Some(geometry) = Geometry::read(record, world) else {
        return;
    };
    box_wires(
        wires,
        geometry.position,
        geometry.rotation,
        geometry.influence,
        false,
    );
    if geometry.projection != geometry.influence || geometry.projection_centre != geometry.position
    {
        box_wires(
            wires,
            geometry.projection_centre,
            geometry.rotation,
            geometry.projection,
            true,
        );
    }
    let radius = (geometry.influence.min_element() * 0.1).clamp(0.02, 0.25);
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        let delta = geometry.rotation * axis * radius;
        wires.push(Segment {
            from: geometry.capture - delta,
            to: geometry.capture + delta,
            tint: Tint::Volume,
        });
    }
    if geometry.capture != geometry.position {
        wires.push(Segment {
            from: geometry.position,
            to: geometry.capture,
            tint: Tint::Volume,
        });
    }
}

fn box_wires(wires: &mut Vec<Segment>, centre: Vec3, rotation: Quat, half: Vec3, dashed: bool) {
    for axis in 0..3 {
        let other = (axis + 1) % 3;
        let last = (axis + 2) % 3;
        for corner in 0..4 {
            let mut from = -half;
            if corner & 1 != 0 {
                from[other] = half[other];
            }
            if corner & 2 != 0 {
                from[last] = half[last];
            }
            let mut to = from;
            to[axis] = half[axis];
            let from = centre + rotation * from;
            let to = centre + rotation * to;
            if dashed {
                for [a, b] in [[0., 1. / 3.], [2. / 3., 1.]] {
                    wires.push(Segment {
                        from: from.lerp(to, a),
                        to: from.lerp(to, b),
                        tint: Tint::Volume,
                    });
                }
            } else {
                wires.push(Segment {
                    from,
                    to,
                    tint: Tint::Volume,
                });
            }
        }
    }
}
