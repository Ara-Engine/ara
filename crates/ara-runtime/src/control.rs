//! Input-driven gameplay: player characters and their follow cameras. Systems read
//! actions from `SimContext` only, so agent injection, devices and replay are equal.
use crate::{
    components::{FollowCamera, PlayerControl},
    follow::{FollowView, Wall},
    math::{Pose, approach_angle},
};
use ara_components::Transform;
use ara_input::{Action, ActionKind, ActionMap, ActionValue, Binding, PlayerId};
use ara_physics::{CharacterInput, CharacterMotion, Collider, PhysicsPose, RigidBody};
use ara_physics2d::{CharacterInput2d, CharacterMotion2d};
use ara_sim::{Identity, SimContext};
use bevy_ecs::prelude::*;
use glam::{Mat3, Mat4, Quat, Vec3};
use std::collections::BTreeMap;

/// Planar movement intent `[right, forward]`.
pub const MOVE: &str = "Move";
/// Camera orbit `[yaw, pitch]` per tick, in units of the rig's sensitivity.
pub const LOOK: &str = "Look";
/// Jump press.
pub const JUMP: &str = "Jump";
/// Sprint hold.
pub const SPRINT: &str = "Sprint";

/// The runtime's action declarations with keyboard/mouse bindings
/// (WASD/arrows, Space, Left Shift; `mouse:motion` is aggregated by the device adapter).
pub fn default_actions() -> ActionMap {
    let mut map = ActionMap::default();
    for (name, kind) in [
        (MOVE, ActionKind::Axis2),
        (LOOK, ActionKind::Axis2),
        (JUMP, ActionKind::Button),
        (SPRINT, ActionKind::Button),
    ] {
        // Names are unique constants; declaration cannot fail.
        let _ = map.declare(Action {
            name: name.into(),
            kind,
        });
    }
    let keys = [
        ("w", MOVE, ActionValue::Axis2([0.0, 1.0])),
        ("s", MOVE, ActionValue::Axis2([0.0, -1.0])),
        ("a", MOVE, ActionValue::Axis2([-1.0, 0.0])),
        ("d", MOVE, ActionValue::Axis2([1.0, 0.0])),
        ("up", MOVE, ActionValue::Axis2([0.0, 1.0])),
        ("down", MOVE, ActionValue::Axis2([0.0, -1.0])),
        ("left", MOVE, ActionValue::Axis2([-1.0, 0.0])),
        ("right", MOVE, ActionValue::Axis2([1.0, 0.0])),
        ("space", JUMP, ActionValue::Button(true)),
        ("leftShift", SPRINT, ActionValue::Button(true)),
    ];
    for (control, action, value) in keys {
        let _ = map.bind(Binding {
            device: "keyboard".into(),
            control: control.into(),
            action: action.into(),
            value,
        });
    }
    map
}

fn axis2(ctx: &SimContext, player: u64, action: &str) -> [f32; 2] {
    match ctx.action(PlayerId(player), action) {
        Some(ActionValue::Axis2(v)) => v,
        _ => [0.0; 2],
    }
}

fn pressed(ctx: &SimContext, player: u64, action: &str) -> bool {
    ctx.action(PlayerId(player), action) == Some(ActionValue::Button(true))
}

/// Applies `Look` to follow-camera rigs.
pub fn steer_cameras(ctx: Res<SimContext>, mut rigs: Query<&mut FollowCamera>) {
    for mut rig in &mut rigs {
        let [x, y] = axis2(&ctx, rig.player, LOOK);
        if x != 0.0 || y != 0.0 {
            rig.yaw = approach_angle(rig.yaw, rig.yaw + x * rig.sensitivity, 180.0);
            rig.pitch = (rig.pitch + y * rig.sensitivity).clamp(-80.0, 80.0);
        }
    }
}

/// Turns `Move`/`Sprint`/`Jump` into character controller input, relative to the
/// player's camera rig yaw, and turns the character toward its movement.
pub fn drive_players(
    ctx: Res<SimContext>,
    rigs: Query<(&Identity, &FollowCamera)>,
    mut players: Query<(
        &mut PlayerControl,
        &mut CharacterInput,
        Option<&CharacterMotion>,
    )>,
) {
    let dt = ctx.dt() as f32;
    for (mut control, mut input, motion) in &mut players {
        let camera_yaw = control
            .camera
            .as_ref()
            .and_then(|c| rigs.iter().find(|(id, _)| id.0 == c.0))
            .map_or(0.0, |(_, rig)| rig.yaw);
        let [x, y] = axis2(&ctx, control.player, MOVE);
        let yaw = camera_yaw.to_radians();
        let forward = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let right = Vec3::new(-yaw.cos(), 0.0, yaw.sin());
        let mut direction = forward * y + right * x;
        if direction.length_squared() > 1.0 {
            direction = direction.normalize();
        }
        let speed = if pressed(&ctx, control.player, SPRINT) {
            control.run_speed
        } else {
            control.walk_speed
        };
        input.desired_velocity = (direction * speed).to_array();
        if direction.length_squared() > 1e-6 {
            let target = direction.x.atan2(direction.z).to_degrees();
            control.yaw = approach_angle(control.yaw, target, control.turn_rate * dt);
        }
        let grounded = motion.is_none_or(|m| m.grounded);
        input.jump_speed = if grounded && pressed(&ctx, control.player, JUMP) {
            control.jump_speed
        } else {
            0.0
        };
    }
}

/// 2D players (`PlayerControl` on a `CharacterController2d`): `Move.x` drives the
/// horizontal speed (`Sprint` runs), `Jump` launches while grounded.
pub fn drive_players_2d(
    ctx: Res<SimContext>,
    mut players: Query<(
        &PlayerControl,
        &mut CharacterInput2d,
        Option<&CharacterMotion2d>,
    )>,
) {
    for (control, mut input, motion) in &mut players {
        let [x, _] = axis2(&ctx, control.player, MOVE);
        let speed = if pressed(&ctx, control.player, SPRINT) {
            control.run_speed
        } else {
            control.walk_speed
        };
        input.desired_velocity = [x.clamp(-1.0, 1.0) * speed, 0.0];
        let grounded = motion.is_none_or(|m| m.grounded);
        input.jump_speed = if grounded && pressed(&ctx, control.player, JUMP) {
            control.jump_speed
        } else {
            0.0
        };
    }
}

/// Transforms and parents of everything but camera rigs, for world matrices.
type Hierarchy<'w, 's> =
    Query<'w, 's, (&'static Transform, Option<&'static ChildOf>), Without<FollowCamera>>;
/// Collider bodies and, once physics has adopted them, their world poses.
type Bodies<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Collider,
        &'static RigidBody,
        Option<&'static PhysicsPose>,
    ),
    Without<FollowCamera>,
>;

/// Places follow cameras behind their targets after physics (orbit yaw/pitch,
/// distance); a boom stops in front of static box colliders ([`FollowView`]). Walls
/// are where physics has them, or (before physics adopts them: the first placement, a
/// mirror client) where their transforms put them.
pub fn place_cameras(
    mut rigs: Query<(&FollowCamera, &mut Transform)>,
    targets: Query<(&Identity, &Transform), Without<FollowCamera>>,
    bodies: Bodies,
    hierarchy: Hierarchy,
) {
    let positions: BTreeMap<&str, Vec3> = targets
        .iter()
        .map(|(id, t)| (id.0.as_str(), Vec3::from_array(t.position)))
        .collect();
    for (rig, mut transform) in &mut rigs {
        let Some(&target) = rig
            .target
            .as_ref()
            .and_then(|t| positions.get(t.0.as_str()))
        else {
            continue;
        };
        let walls = bodies
            .iter()
            .filter(|(_, collider, body, _)| Wall::solid(collider, body))
            .filter_map(|(entity, collider, _, physics)| {
                let (translation, rotation) = match physics {
                    Some(pose) => (
                        Vec3::from_array(pose.position),
                        Quat::from_array(pose.rotation.0),
                    ),
                    None => {
                        let (_, rotation, translation) =
                            world_matrix(entity, &hierarchy)?.to_scale_rotation_translation();
                        (translation, rotation)
                    }
                };
                Some(Wall::new(collider, translation, rotation))
            });
        let view = FollowView::new(rig, target, walls);
        let forward = view.forward;
        // Cameras look along local -Z with +Y up.
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(forward);
        *transform = Pose {
            translation: view.eye(),
            rotation: Quat::from_mat3(&Mat3::from_cols(right, up, -forward)).normalize(),
            scale: Vec3::from_array(transform.scale),
        }
        .to_transform();
    }
}

/// An entity's world matrix from its `Transform` and its parents', as
/// [`crate::sync::world_matrices`] resolves it: a parent without one ends the chain.
fn world_matrix(entity: Entity, hierarchy: &Hierarchy) -> Option<Mat4> {
    let (transform, mut parent) = hierarchy.get(entity).ok()?;
    let mut matrix = Pose::from_transform(transform).matrix();
    while let Some(Ok((transform, next))) = parent.map(|p| hierarchy.get(p.parent())) {
        matrix = Pose::from_transform(transform).matrix() * matrix;
        parent = next;
    }
    Some(matrix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::FollowCamera;
    use ara_physics::{Collider, RigidBody};
    use ara_schema::EntityRef;

    /// A player at the origin and a rig behind it (yaw 0 looks along +Z).
    fn rig(world: &mut World) -> Entity {
        world.spawn((Identity("e_player".into()), Transform::default()));
        world
            .spawn((
                FollowCamera {
                    target: Some(EntityRef("e_player".into())),
                    distance: 3.6,
                    height: 1.5,
                    pitch: 0.,
                    ..Default::default()
                },
                Transform::default(),
            ))
            .id()
    }

    #[test]
    fn a_follow_camera_stops_in_front_of_a_wall_not_a_crate() {
        use ara_physics::MotionType;
        use bevy_ecs::system::RunSystemOnce;
        let mut world = World::new();
        let camera = rig(&mut world);
        // The wall's near face is 1.9 m behind the aim point, through its parent.
        let level = world
            .spawn(Transform {
                position: [0., 0., -1.],
                ..Default::default()
            })
            .id();
        let wall = world
            .spawn((
                Collider {
                    half_extents: [5., 3., 0.1],
                    ..Default::default()
                },
                RigidBody::default(),
                Transform {
                    position: [0., 1.5, -1.],
                    ..Default::default()
                },
                ChildOf(level),
            ))
            .id();
        world.spawn((
            Collider::default(),
            RigidBody {
                motion: MotionType::Dynamic,
                ..Default::default()
            },
            Transform {
                position: [0., 1.5, -1.],
                ..Default::default()
            },
        ));
        let eye = |world: &mut World| {
            world
                .run_system_once(place_cameras)
                .expect("place_cameras runs");
            Vec3::from_array(world.get::<Transform>(camera).expect("rig").position)
        };
        let at = eye(&mut world);
        assert!((at - Vec3::new(0., 1.5, -1.65)).length() < 1e-4, "{at}");
        // Once physics has the wall, its pose is the one that counts.
        world.entity_mut(wall).insert(PhysicsPose {
            position: [0., 1.5, -1.],
            ..Default::default()
        });
        let at = eye(&mut world);
        assert!((at - Vec3::new(0., 1.5, -0.65)).length() < 1e-4, "{at}");
    }

    /// Per-tick cost of `place_cameras` with one rig and many static walls on a 3 m
    /// grid around it, placed by physics (every tick) or by transforms only (the first
    /// placement, mirror clients).
    #[test]
    #[ignore = "benchmark: cargo test -p ara-runtime --lib place_cameras_cost -- --ignored --nocapture"]
    fn place_cameras_cost() {
        for (walls, physics) in [0, 100, 1000, 10_000]
            .into_iter()
            .flat_map(|walls| [(walls, true), (walls, false)])
        {
            let mut world = World::new();
            rig(&mut world);
            for i in 0..walls {
                let position = [
                    (i % 100) as f32 * 3. - 150.,
                    0.,
                    (i / 100) as f32 * 3. - 150.,
                ];
                let mut wall = world.spawn((
                    Collider::default(),
                    RigidBody::default(),
                    Transform {
                        position,
                        ..Default::default()
                    },
                ));
                if physics {
                    wall.insert(PhysicsPose {
                        position,
                        ..Default::default()
                    });
                }
            }
            let mut schedule = Schedule::default();
            schedule.add_systems(place_cameras);
            schedule.run(&mut world);
            let at = std::time::Instant::now();
            let runs = 2000;
            for _ in 0..runs {
                schedule.run(&mut world);
            }
            let us = at.elapsed().as_secs_f64() * 1e6 / runs as f64;
            let by = if physics { "physics" } else { "transforms" };
            println!("{walls:>6} walls ({by:>10}): {us:8.2} us per tick");
        }
    }
}
