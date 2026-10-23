use crate::{Body2d, BodyKind2d, Character2d, Collider2d, Shape2d};
use anyhow::{Result, ensure};
use rapier2d::prelude::*;
pub(crate) fn body_builder(body: &Body2d) -> RigidBodyBuilder {
    let builder = match body.kind {
        BodyKind2d::Fixed => RigidBodyBuilder::fixed(),
        BodyKind2d::Dynamic => RigidBodyBuilder::dynamic(),
        BodyKind2d::Kinematic => RigidBodyBuilder::kinematic_position_based(),
    };
    let builder = builder
        .translation(Vector::new(body.position[0], body.position[1]))
        .rotation(body.rotation)
        .linvel(Vector::new(body.velocity[0], body.velocity[1]))
        .angvel(body.angular_velocity)
        .ccd_enabled(body.ccd);
    if body.lock_rotation {
        builder.lock_rotations()
    } else {
        builder
    }
}
pub(crate) fn validate(
    body: &Body2d,
    collider: &Collider2d,
    character: Option<&Character2d>,
) -> Result<()> {
    ensure!(
        body.position
            .iter()
            .chain(body.velocity.iter())
            .chain([&body.rotation, &body.angular_velocity])
            .all(|v| v.is_finite()),
        "nonfinite body"
    );
    let _ = collider_builder(collider)?;
    if let Some(c) = character {
        ensure!(
            body.kind == BodyKind2d::Kinematic
                && c.translation
                    .iter()
                    .chain([&c.max_slope, &c.step_height, &c.snap])
                    .all(|v| v.is_finite())
                && (0. ..std::f32::consts::FRAC_PI_2).contains(&c.max_slope)
                && c.step_height >= 0.
                && c.snap >= 0.,
            "invalid kinematic controller"
        );
    }
    Ok(())
}
pub(crate) fn collider_builder(c: &Collider2d) -> Result<ColliderBuilder> {
    ensure!(
        c.half_extents
            .iter()
            .chain([
                &c.radius,
                &c.half_height,
                &c.density,
                &c.friction,
                &c.restitution
            ])
            .all(|v| v.is_finite() && *v >= 0.)
            && c.density > 0.
            && c.restitution <= 1.,
        "invalid collider material/size"
    );
    let shape = match c.shape {
        Shape2d::Box => {
            ensure!(c.half_extents.iter().all(|v| *v > 0.), "empty box");
            SharedShape::cuboid(c.half_extents[0], c.half_extents[1])
        }
        Shape2d::Circle => {
            ensure!(c.radius > 0., "empty circle");
            SharedShape::ball(c.radius)
        }
        Shape2d::Capsule => {
            ensure!(c.radius > 0., "empty capsule");
            SharedShape::capsule_y(c.half_height, c.radius)
        }
        Shape2d::Polygon => {
            ensure!(
                (3..=64).contains(&c.points.len())
                    && c.points.iter().flatten().all(|v| v.is_finite()),
                "invalid polygon points"
            );
            let points: Vec<_> = c.points.iter().map(|p| Vector::new(p[0], p[1])).collect();
            SharedShape::convex_hull(&points)
                .ok_or_else(|| anyhow::anyhow!("degenerate polygon"))?
        }
    };
    Ok(ColliderBuilder::new(shape)
        .density(c.density)
        .friction(c.friction)
        .restitution(c.restitution)
        .sensor(c.sensor)
        .collision_groups(InteractionGroups::new(
            Group::from_bits_retain(c.memberships),
            Group::from_bits_retain(c.filter),
            InteractionTestMode::And,
        )))
}
