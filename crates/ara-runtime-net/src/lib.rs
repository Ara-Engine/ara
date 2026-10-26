//! Networked play on the runtime (D19: Host+N and Dedicated+N over `ara-net`).
//!
//! - [`Locomotion`]: the predicted player feature. [`NetMotor`] (millimetres,
//!   millidegrees) is its complete rollback unit and `step` is integer math, so owner
//!   prediction and Lightyear rollback replay it bit-exactly. [`NetPlugin`] steps every
//!   player's motor from its `Move` action at `Stage::Gameplay` (the physics character
//!   is asked for the same velocity, which keeps ground height and pushes), then pins
//!   the character to the motor at `Stage::Late`.
//! - [`NetHost`]: the play world's [`Runtime`] as an `ara_net::Authoritative` inside a
//!   Lightyear server. The server's timeline is the clock; each boundary runs the
//!   runtime's plugins (C#, ...) around the session step; every `Identity` entity
//!   replicates its policy fields (`Transform`, `Velocity`, ...). Clients join through
//!   per-client `Link` proxies (optional simulated conditions, measured bandwidth).
//! - [`NetMirror`]: a client's presentation world, the same snapshot built as a
//!   [`Runtime`] that never steps: the owner's predicted motor, the other players'
//!   interpolated motors and every other entity's interpolated transform are written
//!   in, then animation, world poses and cameras run.
//!
//! Every peer builds its world from the same snapshot after [`avatars`], so record
//! identities match across processes.
mod host;
mod locomotion;
mod mirror;
pub use locomotion::{Locomotion, MotorInput, NetMotor};

pub use ara_net::{Conditions, JoinToken};
pub use host::{HostConfig, HostStats, NetHost};
pub use mirror::{MirrorStats, NetMirror};

use ara_components::Transform;
use ara_doc::{RecordId, Workspace};
use ara_input::{ActionValue, PlayerId};
use ara_net::NetError;
use ara_physics::{CharacterInput, CharacterMotion, Velocity};
use ara_runtime::{
    Runtime,
    components::{FollowCamera, PlayerControl},
    control::{JUMP, LOOK, MOVE, SPRINT},
    plugin::{RuntimePlugin, Stage},
};
use ara_schema::{Schema, SchemaDb};
use ara_sim::{Session, SimContext, SimError, SimRegistry, SimulationFeature};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Errors of networked play.
pub type Result<T> = std::result::Result<T, NetError>;

/// The listen host's (and a local scene's) player.
pub const HOST_PLAYER: u64 = 1;

/// Registers [`NetMotor`] and installs the motor systems. Every peer of a networked
/// world installs it (the schema hash covers the motor); only authorities step it.
pub struct NetPlugin;

impl RuntimePlugin for NetPlugin {
    fn name(&self) -> &'static str {
        "net"
    }

    fn prepare(&mut self, _: &mut World, schema: &mut SchemaDb) -> ara_runtime::Result<()> {
        schema.register::<NetMotor>();
        Ok(())
    }

    fn register(&mut self, registry: &mut SimRegistry) -> ara_runtime::Result<()> {
        registry.register_component::<NetMotor>()?;
        Ok(())
    }

    fn install(&mut self, stage: Stage, session: &mut Session) -> ara_runtime::Result<()> {
        match stage {
            Stage::Gameplay => session.add_system("net.motors", motors)?,
            Stage::Late => session.add_system("net.pin", pin)?,
        }
        Ok(())
    }
}

/// Steps each player's complete predicted controls and sends ground velocity/jump
/// to the physics character, which supplies authoritative height and contacts.
fn motors(
    ctx: Res<SimContext>,
    mut players: Query<(
        &PlayerControl,
        &mut NetMotor,
        Option<&mut CharacterInput>,
        Option<&CharacterMotion>,
    )>,
    mut rigs: Query<&mut FollowCamera>,
) {
    let hz = (1.0 / ctx.dt()) as f32;
    for (control, mut motor, input, motion) in &mut players {
        let actions = [MOVE, LOOK, SPRINT, JUMP]
            .into_iter()
            .filter_map(|name| {
                ctx.action(PlayerId(control.player), name)
                    .map(|v| (name.into(), v))
            })
            .collect();
        let typed = locomotion::sample(&actions).unwrap_or_default();
        let before = *motor;
        Locomotion::step(&mut motor, &typed);
        for mut rig in &mut rigs {
            if rig.player == control.player {
                rig.yaw = motor.camera_yaw as f32 / 1000.0;
                rig.pitch = motor.camera_pitch as f32 / 1000.0;
            }
        }
        if let Some(mut input) = input {
            let metres = |mm: i32| mm as f32 / 1000.0 * hz;
            input.desired_velocity = [metres(motor.x - before.x), 0.0, metres(motor.z - before.z)];
            input.jump_speed =
                if typed.jump && !before.jump_held && motion.is_none_or(|m| m.grounded) {
                    control.jump_speed
                } else {
                    0.0
                };
        }
    }
}

type PinnedPlayer = (
    &'static mut NetMotor,
    &'static mut PlayerControl,
    &'static mut Transform,
    Option<&'static CharacterMotion>,
    Option<&'static Velocity>,
);

/// Reconciles motors with authoritative character contacts. Prediction remains
/// contact-free between snapshots; it must never teleport the server through walls.
fn pin(ctx: Res<SimContext>, mut players: Query<PinnedPlayer>) {
    for (mut motor, mut control, mut transform, motion, velocity) in &mut players {
        motor.y = (transform.position[1] * 1000.0).round() as i32;
        if let Some(motion) = motion {
            motor.grounded = motion.grounded;
            if motion.grounded {
                motor.floor = motor.y;
                motor.vertical_speed = 0;
            } else if let Some(velocity) = velocity {
                motor.vertical_speed =
                    (velocity.linear[1] * 1_000_000.0 * ctx.dt() as f32).round() as i32;
            }
        }
        let [x, z] = motor.ground();
        let yaw = motor.yaw();
        if motion.is_some() {
            motor.x = (transform.position[0] * 1000.0).round() as i32;
            motor.z = (transform.position[2] * 1000.0).round() as i32;
        } else if transform.position[0] != x || transform.position[2] != z {
            transform.position[0] = x;
            transform.position[2] = z;
        }
        if control.yaw != yaw {
            control.yaw = yaw;
            transform.rotation = [0.0, yaw, 0.0];
        }
    }
}

/// Inserts a [`NetMotor`] on every `PlayerControl` entity that lacks one (both the
/// authority and the mirror call this once, after building).
pub fn attach_motors(runtime: &mut Runtime) -> Vec<(String, u64)> {
    let hz = runtime.hz();
    let world = runtime.play_mut().session_mut().world_mut();
    let rigs: std::collections::BTreeMap<String, FollowCamera> = world
        .query::<(&ara_sim::Identity, &FollowCamera)>()
        .iter(world)
        .map(|(id, rig)| (id.0.clone(), rig.clone()))
        .collect();
    let mut found = Vec::new();
    let mut query = world.query_filtered::<(
        Entity,
        &ara_sim::Identity,
        &PlayerControl,
        &Transform,
    ), Without<NetMotor>>();
    for (entity, identity, control, transform) in query.iter(world) {
        let mut motor = NetMotor::at(transform, control, hz);
        if let Some(rig) = control
            .camera
            .as_ref()
            .and_then(|id| rigs.get(&id.0))
            .or_else(|| rigs.values().find(|rig| rig.player == control.player))
            .or_else(|| rigs.values().next())
        {
            motor.camera_yaw = (rig.yaw * 1000.0).round() as i32;
            motor.camera_pitch = (rig.pitch * 1000.0).round() as i32;
            motor.sensitivity = (rig.sensitivity * 1000.0).round() as i32;
        }
        found.push((entity, identity.0.clone(), control.player, motor));
    }
    let mut players = Vec::new();
    for (entity, identity, player, motor) in found {
        world.entity_mut(entity).insert(motor);
        players.push((identity, player));
    }
    players.sort();
    players
}

/// Play-only avatars for networked play: copies the scene's player-1 record (the
/// `PlayerControl` whose `player` is 1, the default) as `e_player<NN>` for players
/// `2..=players`, 1.5 m apart along X, with fresh physics ids and no camera. Every
/// peer applies the same copies to the same snapshot (identities match across
/// processes); the authoring documents never see them. Returns the avatar ids.
pub fn avatars(
    workspace: &mut Workspace,
    scene: &str,
    players: u64,
) -> std::result::Result<Vec<String>, String> {
    let doc = workspace
        .doc_mut(scene)
        .ok_or_else(|| format!("scene {scene} is not in the snapshot"))?;
    let (id, record) = doc
        .records()
        .iter()
        .find(|(_, r)| {
            r.get("PlayerControl").is_some_and(|c| {
                c.get("player")
                    .and_then(Value::as_u64)
                    .unwrap_or(HOST_PLAYER)
                    == HOST_PLAYER
            })
        })
        .map(|(id, r)| (id.clone(), r.clone()))
        .ok_or("networked play needs a player (a PlayerControl entity) in the scene")?;
    let mut ids = vec![id.as_str().to_owned()];
    for player in 2..=players {
        let mut copy = record.clone();
        let offset = 1.5 * (player - 1) as f64;
        let control = copy.entry("PlayerControl").or_insert_with(|| json!({}));
        if let Some(control) = control.as_object_mut() {
            control.insert("player".into(), json!(player));
            control.remove("camera");
        }
        let transform = copy.entry("Transform").or_insert_with(|| json!({}));
        if let Some(transform) = transform.as_object_mut() {
            let position = transform
                .entry("position")
                .or_insert_with(|| json!([0, 0, 0]));
            if let Some(x) = position.get_mut(0) {
                *x = json!(x.as_f64().unwrap_or(0.0) + offset);
            }
        }
        if let Some(physics) = copy.get_mut("PhysicsId").and_then(Value::as_object_mut) {
            physics.insert("value".into(), json!(9_000 + player));
        }
        let name = copy.get("name").and_then(Value::as_str).unwrap_or("Player");
        let name = format!("{name} {player}");
        copy.insert("name".into(), json!(name));
        copy.remove("order");
        let rid = RecordId::parse(&format!("e_player{player:02}"))?;
        ids.push(rid.as_str().to_owned());
        doc.insert_unchecked(rid, copy);
    }
    Ok(ids)
}
