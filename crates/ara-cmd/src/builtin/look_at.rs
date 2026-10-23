//! Camera/light aiming uses the runtime's intrinsic XYZ rotation and local -Z axis.

use ara_components::Transform;
use ara_diag::{Diagnostic, codes};
use ara_doc::prefab::Resolved;
use ara_schema::{EntityRef, FieldPath, Schema};
use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Changed;
use crate::{Command, EditCtx, Hints};

/// Parameters of `transform.lookAt`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransformLookAtParams {
    /// Camera, light or other entity whose local -Z axis should face the target.
    pub entity: EntityRef,
    /// World-space target position. Supply exactly one of point or target.
    #[ara(unit = Meters)]
    pub point: Option<[f32; 3]>,
    /// Target entity in this document; its world position is used.
    pub target: Option<EntityRef>,
    /// Preferred world-space up vector; defaults to +Y. At a pole a stable perpendicular axis is used.
    pub up: Option<[f32; 3]>,
}

/// Aim an entity through ordinary, undoable transform ops, including prefab overrides.
pub struct TransformLookAt;

impl Command for TransformLookAt {
    type Params = TransformLookAtParams;
    type Output = Changed;
    const NAME: &'static str = "transform.lookAt";
    const SUMMARY: &'static str =
        "Aim local -Z at a world point or entity with a preferred world up vector.";
    const HINTS: Hints = Hints {
        idempotent: true,
        destructive: false,
    };

    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_camera","point":[0,1,0],"up":[0,1,0]})]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Changed, Diagnostic> {
        let read = ctx.read();
        let entity = read.target(&p.entity.0)?;
        let resolved = read.resolved();
        let world = world_matrix(&resolved, &entity.address())?;
        let point = match (p.point, p.target) {
            (Some(point), None) => Vec3::from_array(point),
            (None, Some(target)) => {
                let target = read.target(&target.0)?;
                world_matrix(&resolved, &target.address())?.transform_point3(Vec3::ZERO)
            }
            _ => return Err(invalid("supply exactly one of point or target")),
        };
        let parent = resolved
            .get(&entity.address())
            .and_then(|e| e.parent.as_deref());
        let parent = parent
            .map(|id| world_matrix(&resolved, id))
            .transpose()?
            .unwrap_or(Mat4::IDENTITY);
        if !parent.determinant().is_finite() || parent.determinant().abs() < 1e-12 {
            return Err(invalid("cannot aim beneath a singular parent transform"));
        }
        let inverse = parent.inverse();
        let direction = inverse.transform_vector3(point - world.transform_point3(Vec3::ZERO));
        let up = Vec3::from_array(p.up.unwrap_or([0., 1., 0.]));
        if !up.is_finite() || up.length_squared() < 1e-12 {
            return Err(invalid("up must be a finite nonzero vector"));
        }
        let rotation = aim(direction, inverse.transform_vector3(up))?;
        if !read.has_component(&entity, "Transform") {
            ctx.set_entity(&entity, &FieldPath::field("Transform"), Some(json!({})))?;
        }
        ctx.set_entity(
            &entity,
            &FieldPath::field("Transform").child("rotation"),
            Some(ara_json::to_value(&rotation).map_err(|e| invalid(e.to_string()))?),
        )?;
        Ok(Changed {
            entity: entity.address(),
        })
    }
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}

/// Compose parents iteratively, so broken hierarchies diagnose instead of recursing forever.
fn world_matrix(entities: &Resolved, address: &str) -> Result<Mat4, Diagnostic> {
    let mut matrix = Mat4::IDENTITY;
    let mut current = Some(address);
    for _ in 0..=entities.len() {
        let Some(id) = current else { return Ok(matrix) };
        let entity = entities
            .get(id)
            .ok_or_else(|| invalid(format!("missing entity `{id}`")))?;
        let transform = entity
            .record
            .get("Transform")
            .cloned()
            .map(serde_json::from_value::<Transform>)
            .transpose()
            .map_err(|e| invalid(e.to_string()))?
            .unwrap_or_default();
        let [x, y, z] = transform.rotation.map(f32::to_radians);
        matrix = Mat4::from_scale_rotation_translation(
            Vec3::from_array(transform.scale),
            Quat::from_euler(EulerRot::XYZ, x, y, z),
            Vec3::from_array(transform.position),
        ) * matrix;
        current = entity.parent.as_deref();
    }
    Err(invalid("cyclic transform hierarchy"))
}

fn aim(direction: Vec3, up: Vec3) -> Result<[f32; 3], Diagnostic> {
    if !direction.is_finite() || direction.length_squared() < 1e-12 {
        return Err(invalid(
            "target must be finite and distinct from the entity position",
        ));
    }
    let forward = direction.normalize();
    let mut up = up.normalize();
    if forward.cross(up).length_squared() < 1e-8 {
        up = if forward.x.abs() < 0.9 {
            Vec3::X
        } else {
            Vec3::Z
        };
    }
    let right = forward.cross(up).normalize();
    let up = right.cross(forward);
    let rotation = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
    let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
    Ok([x, y, z].map(f32::to_degrees))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aims_in_all_quadrants_and_at_poles_without_flipping_up() {
        for direction in [
            Vec3::NEG_Z,
            Vec3::Z,
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::new(1., 0.4, 2.),
            Vec3::new(-1., -0.4, 2.),
            Vec3::new(0., 1., 1e-7),
        ] {
            let [x, y, z] = aim(direction, Vec3::Y)
                .expect("valid direction")
                .map(f32::to_radians);
            let rotation = Quat::from_euler(EulerRot::XYZ, x, y, z);
            assert!((rotation * Vec3::NEG_Z).abs_diff_eq(direction.normalize(), 1e-5));
            assert!((rotation * Vec3::Y).dot(Vec3::Y) >= -1e-5);
        }
        assert!(aim(Vec3::ZERO, Vec3::Y).is_err());
        assert!(aim(Vec3::NAN, Vec3::Y).is_err());
    }
}
