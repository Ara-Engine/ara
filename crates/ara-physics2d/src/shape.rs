//! Validation and Rapier builders. Every value is checked before any solver mutation.
use crate::{
    CharacterController2d, CharacterInput2d, Collider2d, ColliderKind2d, MotionType2d,
    PhysicsPose2d, RigidBody2d, Velocity2d,
};
use anyhow::{Result, ensure};
use rapier2d::prelude::*;

fn finite<'a>(values: impl IntoIterator<Item = &'a f32>) -> bool {
    values.into_iter().all(|v| v.is_finite())
}

/// Collision groups for a 0..15 layer under the configured symmetric inclusion masks.
pub(crate) fn groups(layer: u32, masks: &[u32; 16]) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_retain(1 << layer),
        Group::from_bits_retain(masks[layer as usize]),
        InteractionTestMode::And,
    )
}

/// Checks state components shared by bodies and characters.
pub(crate) fn validate_state(pose: &PhysicsPose2d, velocity: &Velocity2d) -> Result<()> {
    ensure!(
        finite(&pose.position)
            && pose.rotation.is_finite()
            && finite(&velocity.linear)
            && velocity.angular.is_finite(),
        "nonfinite physics pose/velocity"
    );
    Ok(())
}

pub(crate) fn validate_body(body: &RigidBody2d) -> Result<()> {
    ensure!(
        body.mass.is_finite() && body.mass >= 0.,
        "rigid body mass must be finite and nonnegative"
    );
    Ok(())
}

pub(crate) fn validate_character(c: &CharacterController2d) -> Result<()> {
    ensure!(
        finite([
            &c.radius,
            &c.height,
            &c.max_slope_deg,
            &c.step_height,
            &c.stick_down,
            &c.padding
        ]) && c.radius > 0.
            && c.height > c.radius * 2.
            && (0.1..=89.9).contains(&c.max_slope_deg)
            && c.step_height >= 0.
            && c.stick_down >= 0.
            && c.padding > 0.
            && (1..=15).contains(&c.layer),
        "invalid character controller"
    );
    Ok(())
}

pub(crate) fn validate_input(input: &CharacterInput2d) -> Result<()> {
    ensure!(
        finite(&input.desired_velocity) && input.jump_speed.is_finite() && input.jump_speed >= 0.,
        "invalid character input"
    );
    Ok(())
}

pub(crate) fn body_builder(
    body: &RigidBody2d,
    pose: &PhysicsPose2d,
    velocity: &Velocity2d,
) -> RigidBodyBuilder {
    let builder = match body.motion {
        MotionType2d::Static => RigidBodyBuilder::fixed(),
        MotionType2d::Dynamic => RigidBodyBuilder::dynamic(),
        MotionType2d::Kinematic => RigidBodyBuilder::kinematic_position_based(),
    };
    let builder = builder
        .translation(Vector::new(pose.position[0], pose.position[1]))
        .rotation(pose.rotation)
        .linvel(Vector::new(velocity.linear[0], velocity.linear[1]))
        .angvel(velocity.angular)
        .can_sleep(body.allow_sleep)
        .ccd_enabled(body.continuous);
    if body.lock_rotation {
        builder.lock_rotations()
    } else {
        builder
    }
}

/// Builds a validated collider. Layer zero resolves to static (0) or moving (1).
pub(crate) fn collider_builder(
    c: &Collider2d,
    body: &RigidBody2d,
    masks: &[u32; 16],
) -> Result<ColliderBuilder> {
    ensure!(
        finite(c.half_extents.iter().chain([
            &c.radius,
            &c.half_height,
            &c.friction,
            &c.restitution
        ])) && c.half_height >= 0.
            && c.friction >= 0.
            && (0. ..=1.).contains(&c.restitution)
            && c.layer <= 15,
        "invalid collider material/size/layer"
    );
    let shape = match c.kind {
        ColliderKind2d::Box => {
            ensure!(c.half_extents.iter().all(|v| *v > 0.), "empty box");
            SharedShape::cuboid(c.half_extents[0], c.half_extents[1])
        }
        ColliderKind2d::Circle => {
            ensure!(c.radius > 0., "empty circle");
            SharedShape::ball(c.radius)
        }
        ColliderKind2d::Capsule => {
            ensure!(c.radius > 0., "empty capsule");
            SharedShape::capsule_y(c.half_height, c.radius)
        }
        ColliderKind2d::Polygon => {
            ensure!(
                (3..=64).contains(&c.points.len())
                    && c.points.iter().flatten().all(|v| v.is_finite()),
                "polygon needs 3..64 finite points"
            );
            let points: Vec<_> = c.points.iter().map(|p| Vector::new(p[0], p[1])).collect();
            SharedShape::convex_hull(&points)
                .ok_or_else(|| anyhow::anyhow!("degenerate polygon"))?
        }
    };
    let layer = match (c.layer, body.motion) {
        (0, MotionType2d::Static) => 0,
        (0, _) => 1,
        (layer, _) => layer,
    };
    let builder = ColliderBuilder::new(shape)
        .friction(c.friction)
        .restitution(c.restitution)
        .sensor(c.sensor)
        .collision_groups(groups(layer, masks));
    Ok(if body.mass > 0. {
        builder.mass(body.mass)
    } else {
        builder.density(1.)
    })
}

/// Capsule shape and collision groups owned by a character controller.
pub(crate) fn character_collider(c: &CharacterController2d, masks: &[u32; 16]) -> ColliderBuilder {
    ColliderBuilder::capsule_y(c.height * 0.5 - c.radius, c.radius)
        .collision_groups(groups(c.layer, masks))
}

/// Validates collider geometry and material without retaining a builder.
pub(crate) fn validate_collider(c: &Collider2d, body: &RigidBody2d) -> Result<()> {
    collider_builder(c, body, &[0xffff; 16]).map(drop)
}
