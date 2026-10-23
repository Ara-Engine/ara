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
mod mirror;

pub use ara_net::{Conditions, JoinToken};
pub use host::{HostConfig, HostStats, NetHost};
pub use mirror::{MirrorStats, NetMirror};

use ara_components::Transform;
use ara_doc::{RecordId, Workspace};
use ara_input::{ActionValue, PlayerId};
use ara_net::NetError;
use ara_physics::CharacterInput;
use ara_runtime::{
    Runtime,
    components::PlayerControl,
    control::MOVE,
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

/// Networked player locomotion: ground-plane position and facing in fixed point, the
/// complete state a client predicts for its own player. Runtime-only: inserted when a
/// networked world starts, from the entity's `Transform` and `PlayerControl`.
#[derive(Component, Schema, Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[ara(component, module = "net")]
#[serde(default, rename_all = "camelCase")]
pub struct NetMotor {
    /// World X in millimetres.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub x: i32,
    /// World Z in millimetres.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub z: i32,
    /// Facing in millidegrees; 0 faces +Z.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub heading: i32,
    /// Ground speed in millimetres per tick.
    #[ara(replicate(audience = Relevant, rollback))]
    pub speed: i32,
}

/// Largest facing change per tick, in millidegrees (540 degrees/s at 60 Hz).
const TURN: i32 = 9_000;

/// Facing for each `[x, z]` direction, in millidegrees (`atan2(x, z)`).
fn heading_of(x: i8, z: i8) -> i32 {
    match (x.signum(), z.signum()) {
        (0, 1) => 0,
        (1, 1) => 45_000,
        (1, 0) => 90_000,
        (1, -1) => 135_000,
        (0, -1) => 180_000,
        (-1, -1) => -135_000,
        (-1, 0) => -90_000,
        _ => -45_000,
    }
}

/// The signed shortest turn from `from` to `to`, in millidegrees (-180°, 180°].
fn turn(from: i32, to: i32) -> i32 {
    let delta = (to - from).rem_euclid(360_000);
    if delta > 180_000 {
        delta - 360_000
    } else {
        delta
    }
}

/// Predicted player locomotion: `Move` sampled to eight world-space directions.
pub struct Locomotion;

impl SimulationFeature for Locomotion {
    type State = NetMotor;
    type Input = [i8; 2];
    const ACTION: &'static str = MOVE;

    fn input(action: Option<ActionValue>) -> std::result::Result<[i8; 2], SimError> {
        match action {
            None => Ok([0; 2]),
            Some(ActionValue::Axis2([x, z])) if x.is_finite() && z.is_finite() => Ok([
                x.clamp(-1.0, 1.0).round() as i8,
                z.clamp(-1.0, 1.0).round() as i8,
            ]),
            _ => Err(SimError("Move must be a finite Axis2".into())),
        }
    }

    fn action(input: &[i8; 2]) -> std::result::Result<ActionValue, SimError> {
        if input.iter().any(|v| !(-1..=1).contains(v)) {
            return Err(SimError("movement input outside [-1, 1]".into()));
        }
        Ok(ActionValue::Axis2([
            f32::from(input[0]),
            f32::from(input[1]),
        ]))
    }

    fn step(state: &mut NetMotor, input: &[i8; 2]) {
        let [x, z] = *input;
        if x == 0 && z == 0 {
            return;
        }
        // Diagonals keep ground speed: 707/1000 per axis, exact integer math.
        let axis = if x != 0 && z != 0 {
            state.speed * 707 / 1000
        } else {
            state.speed
        };
        state.x += i32::from(x) * axis;
        state.z += i32::from(z) * axis;
        let delta = turn(state.heading, heading_of(x, z));
        state.heading = turn(0, state.heading + delta.clamp(-TURN, TURN));
    }

    fn interpolate(a: NetMotor, b: NetMotor, alpha: f32) -> NetMotor {
        let lerp = |a: i32, b: i32| a + ((b - a) as f32 * alpha).round() as i32;
        NetMotor {
            x: lerp(a.x, b.x),
            z: lerp(a.z, b.z),
            heading: turn(
                0,
                a.heading + (turn(a.heading, b.heading) as f32 * alpha).round() as i32,
            ),
            speed: b.speed,
        }
    }

    /// Ground-plane position error in millimetres.
    fn correction(a: &NetMotor, b: &NetMotor) -> f64 {
        f64::from(a.x - b.x).hypot(f64::from(a.z - b.z))
    }

    fn position(state: &NetMotor) -> Option<[f32; 2]> {
        Some([state.x as f32 / 1000.0, state.z as f32 / 1000.0])
    }
}

impl NetMotor {
    /// The motor at an entity's pose, walking at `control.walk_speed` on an `hz` tick.
    pub fn at(transform: &Transform, control: &PlayerControl, hz: u32) -> Self {
        let mm = |m: f32| (m * 1000.0).round() as i32;
        Self {
            x: mm(transform.position[0]),
            z: mm(transform.position[2]),
            heading: turn(0, (control.yaw * 1000.0).round() as i32),
            speed: mm(control.walk_speed / hz.max(1) as f32),
        }
    }

    /// World position (metres) on the ground plane.
    pub fn ground(&self) -> [f32; 2] {
        [self.x as f32 / 1000.0, self.z as f32 / 1000.0]
    }

    /// Facing in degrees.
    pub fn yaw(&self) -> f32 {
        self.heading as f32 / 1000.0
    }
}

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

/// Steps each player's motor from its `Move` action; the physics character is asked
/// for the same ground velocity (it supplies height and contacts, never position).
fn motors(
    ctx: Res<SimContext>,
    mut players: Query<(&PlayerControl, &mut NetMotor, Option<&mut CharacterInput>)>,
) {
    let hz = (1.0 / ctx.dt()) as f32;
    for (control, mut motor, input) in &mut players {
        let typed =
            Locomotion::input(ctx.action(PlayerId(control.player), MOVE)).unwrap_or_default();
        let before = *motor;
        Locomotion::step(&mut motor, &typed);
        if let Some(mut input) = input {
            let metres = |mm: i32| mm as f32 / 1000.0 * hz;
            input.desired_velocity = [metres(motor.x - before.x), 0.0, metres(motor.z - before.z)];
            input.jump_speed = 0.0;
        }
    }
}

/// Pins players to their motors after physics: ground X/Z and facing come from the
/// motor, height from the character. The next push teleports the body to match.
fn pin(mut players: Query<(&NetMotor, &mut PlayerControl, &mut Transform)>) {
    for (motor, mut control, mut transform) in &mut players {
        let [x, z] = motor.ground();
        let yaw = motor.yaw();
        if transform.position[0] != x || transform.position[2] != z {
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
    let mut found = Vec::new();
    let mut query = world.query_filtered::<(
        Entity,
        &ara_sim::Identity,
        &PlayerControl,
        &Transform,
    ), Without<NetMotor>>();
    for (entity, identity, control, transform) in query.iter(world) {
        found.push((
            entity,
            identity.0.clone(),
            control.player,
            NetMotor::at(transform, control, hz),
        ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motor_steps_are_integer_and_keep_diagonal_speed() {
        let mut motor = NetMotor {
            speed: 50,
            ..Default::default()
        };
        Locomotion::step(&mut motor, &[0, 1]);
        assert_eq!((motor.x, motor.z, motor.heading), (0, 50, 0));
        Locomotion::step(&mut motor, &[1, 1]);
        assert_eq!((motor.x, motor.z, motor.heading), (35, 85, 9_000));
        let mut turned = NetMotor::default();
        for _ in 0..40 {
            Locomotion::step(&mut turned, &[0, -1]);
        }
        assert_eq!(turned.heading, 180_000);
        assert_eq!(turn(170_000, -170_000), 20_000);
    }

    #[test]
    fn interpolation_takes_the_short_arc() {
        let a = NetMotor {
            heading: 170_000,
            ..Default::default()
        };
        let b = NetMotor {
            heading: -170_000,
            x: 100,
            ..Default::default()
        };
        let mid = Locomotion::interpolate(a, b, 0.5);
        assert_eq!((mid.x, mid.heading), (50, 180_000));
    }
}
