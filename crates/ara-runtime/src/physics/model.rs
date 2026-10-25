//! Model-space cooked geometry resolves outside frame extraction; native handles
//! are recreated with each physics world and replaced when their source changes.
use crate::{Result, RuntimeError, animation::Library, assets::asset_id};
use ara_asset::Digest;
use ara_physics::{
    Collider, ColliderKind, CookedCollider, MotionType, PhysicsBackend, PhysicsWorld, RigidBody,
    Shape,
};
use ara_physics_jolt::JoltBackend;
use bevy_ecs::prelude::*;
use glam::Vec3;

#[derive(Component)]
pub(super) struct Generation {
    collider: Collider,
    /// None keeps a bounded placeholder until the first model cook arrives.
    digest: Option<Digest>,
    scale: Vec3,
}

pub(super) fn cook(world: &mut World) -> Result<()> {
    let mut wanted = Vec::new();
    let mut query = world.query::<(
        Entity,
        &Collider,
        Option<&Generation>,
        Option<&CookedCollider>,
    )>();
    for (entity, collider, generation, cooked) in query.iter(world) {
        if !matches!(
            collider.kind,
            ColliderKind::ModelConvex | ColliderKind::ModelMesh
        ) {
            continue;
        }
        let reference = collider
            .asset
            .as_ref()
            .ok_or_else(|| RuntimeError("model collider requires Collider.asset".into()))?;
        let id = asset_id(reference)?;
        let model = world
            .get_resource::<Library>()
            .and_then(|library| library.models.get(id));
        let scale = world_scale(world, entity)?;
        if cooked.is_some()
            && generation.is_some_and(|g| {
                g.collider.kind == collider.kind
                    && g.collider.asset == collider.asset
                    && g.collider.node == collider.node
                    && g.collider.primitive == collider.primitive
                    && g.digest == model.map(|model| model.digest)
                    && (model.is_some() || g.collider.half_extents == collider.half_extents)
                    && same_scale(g.scale, scale)
            })
        {
            continue;
        }
        wanted.push((entity, collider.clone(), scale));
    }
    for (entity, collider, scale) in wanted {
        if collider.kind == ColliderKind::ModelMesh
            && world
                .get::<RigidBody>(entity)
                .is_some_and(|b| b.motion != MotionType::Static)
        {
            return Err(RuntimeError(
                "model mesh collision requires a static RigidBody".into(),
            ));
        }
        if !scale.is_finite() || scale.abs().min_element() <= 1e-8 {
            return Err(RuntimeError(
                "model collider scale must be finite and nonzero".into(),
            ));
        }
        let reference = collider
            .asset
            .as_ref()
            .ok_or_else(|| RuntimeError("model collider requires an asset".into()))?;
        let id = asset_id(reference)?;
        let model = world
            .get_resource::<Library>()
            .and_then(|library| library.models.get(id));
        let Some(model) = model else {
            // Match presentation's first-cook placeholder policy. Keep the ECS
            // body and its authored centre, but cook conservative box bounds;
            // None distinguishes this from a real generation and avoids doing
            // work again until the asset, selection or world scale changes.
            let half_extents = (Vec3::from_array(collider.half_extents) * scale.abs()).to_array();
            let handle = world
                .resource_mut::<PhysicsWorld<JoltBackend>>()
                .backend_mut()
                .create_shape(Shape::Box { half_extents })?;
            world.entity_mut(entity).insert((
                CookedCollider(handle),
                Generation {
                    collider,
                    digest: None,
                    scale,
                },
            ));
            continue;
        };
        let selection = model.select(collider.node.as_deref(), collider.primitive)?;
        let offset = Vec3::from_array(selection.offset);
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for index in selection.primitives {
            let primitive = &model.primitives[index];
            let base = u32::try_from(vertices.len())
                .map_err(|_| RuntimeError("model collider exceeds index capacity".into()))?;
            vertices.extend(
                primitive
                    .vertices
                    .iter()
                    .map(|v| ((Vec3::from_array(v.position) + offset) * scale).to_array()),
            );
            for &index in &primitive.indices {
                indices.push(
                    base.checked_add(index)
                        .ok_or_else(|| RuntimeError("model collider index overflow".into()))?,
                );
            }
        }
        if scale.x * scale.y * scale.z < 0. {
            for triangle in indices.as_chunks_mut::<3>().0 {
                triangle.swap(1, 2);
            }
        }
        if vertices.is_empty() || indices.is_empty() {
            return Err(RuntimeError(
                "selected model collider has no triangles".into(),
            ));
        }
        let digest = model.digest;
        let shape = if collider.kind == ColliderKind::ModelConvex {
            Shape::ConvexHull {
                vertices: &vertices,
            }
        } else {
            Shape::Mesh {
                vertices: &vertices,
                indices: &indices,
            }
        };
        let handle = world
            .resource_mut::<PhysicsWorld<JoltBackend>>()
            .backend_mut()
            .create_shape(shape)?;
        world.entity_mut(entity).insert((
            CookedCollider(handle),
            Generation {
                collider,
                digest: Some(digest),
                scale,
            },
        ));
    }
    Ok(())
}

fn same_scale(a: Vec3, b: Vec3) -> bool {
    // Decomposing a rotating hierarchy introduces roundoff; it must not recook
    // an otherwise rigid collider every tick. Preserve mirror sign changes.
    let tolerance = a.abs().max(b.abs()).max(Vec3::splat(1e-6)) * 1e-5;
    a.signum() == b.signum() && (a - b).abs().cmple(tolerance).all()
}

pub(super) fn reset(world: &mut World) {
    let mut query = world.query_filtered::<Entity, With<Generation>>();
    let entities: Vec<_> = query.iter(world).collect();
    for entity in entities {
        world
            .entity_mut(entity)
            .remove::<(CookedCollider, Generation)>();
    }
}

fn world_scale(world: &World, entity: Entity) -> Result<Vec3> {
    let mut cursor = Some(entity);
    let mut matrix = glam::Mat4::IDENTITY;
    let mut depth = 0;
    while let Some(entity) = cursor {
        if depth >= 128 {
            return Err(RuntimeError(
                "model collider hierarchy exceeds 128 levels".into(),
            ));
        }
        if let Some(transform) = world.get::<ara_components::Transform>(entity) {
            matrix = crate::math::Pose::from_transform(transform).matrix() * matrix;
        }
        cursor = world.get::<ChildOf>(entity).map(ChildOf::parent);
        depth += 1;
    }
    Ok(crate::math::Pose::from_matrix(matrix).scale)
}
