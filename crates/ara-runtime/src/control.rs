//! Input-driven gameplay: player characters and their follow cameras. Systems read
//! actions from `SimContext` only, so agent injection, devices and replay are equal.
use crate::{
    components::{FollowCamera, PlayerControl},
    math::{Pose, approach_angle},
};
use ara_components::Transform;
use ara_input::{Action, ActionKind, ActionMap, ActionValue, Binding, PlayerId};
use ara_physics::{CharacterInput, CharacterMotion};
use ara_physics2d::{CharacterInput2d, CharacterMotion2d};
use ara_sim::{Identity, SimContext};
use bevy_ecs::prelude::*;
use glam::{Mat3, Quat, Vec3};
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

/// Places follow cameras behind their targets after physics (orbit yaw/pitch, distance).
pub fn place_cameras(
    mut rigs: Query<(&FollowCamera, &mut Transform)>,
    targets: Query<(&Identity, &Transform), Without<FollowCamera>>,
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
        let (yaw, pitch) = (rig.yaw.to_radians(), rig.pitch.to_radians());
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let aim = target + Vec3::Y * rig.height;
        let eye = aim - forward * rig.distance;
        // Cameras look along local -Z with +Y up.
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(forward);
        *transform = Pose {
            translation: eye,
            rotation: Quat::from_mat3(&Mat3::from_cols(right, up, -forward)).normalize(),
            scale: Vec3::from_array(transform.scale),
        }
        .to_transform();
    }
}
