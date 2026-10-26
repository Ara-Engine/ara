//! Complete predicted movement state and a bounded multi-action input codec.
use super::*;
use ara_runtime::control::{JUMP, LOOK, SPRINT, INTERACT, INVENTORY};
use std::collections::BTreeMap;

/// Every control sampled into one native prediction-history entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MotorInput {
    /// Camera-relative right/forward directions.
    pub movement: [i8; 2],
    /// Camera orbit in millidegrees before rig sensitivity is applied.
    pub look: [i32; 2],
    /// Run while held.
    pub sprint: bool,
    /// Jump while grounded; a hold requires release before another jump.
    pub jump: bool,
    /// Authoritative interaction hold; gameplay edge-detects in its ECS component.
    pub interact: bool,
    /// Owner inventory display toggle; gameplay edge-detects in ECS.
    pub inventory: bool,
}

/// Complete owner rollback state. Positions are millimetres, camera/facing angles
/// millidegrees, and vertical rates micrometres per tick. Physics supplies authority
/// corrections for terrain/contact changes; replay uses only these ECS fields.
#[derive(Component, Schema, Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[ara(component, module = "net")]
#[serde(default, rename_all = "camelCase")]
pub struct NetMotor {
    /// World X in millimetres.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub x: i32,
    /// World Y in millimetres.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub y: i32,
    /// World Z in millimetres.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub z: i32,
    /// Facing in millidegrees; zero faces +Z.
    #[ara(replicate(audience = Relevant, interpolate = Linear, rollback))]
    pub heading: i32,
    /// Walk speed in millimetres per tick.
    #[ara(replicate(audience = Relevant, rollback))]
    pub speed: i32,
    /// Sprint speed in millimetres per tick.
    #[ara(replicate(audience = Relevant, rollback))]
    pub run_speed: i32,
    /// Camera orbit yaw in millidegrees.
    #[ara(replicate(audience = Relevant, rollback))]
    pub camera_yaw: i32,
    /// Camera orbit pitch in millidegrees.
    #[ara(replicate(audience = Relevant, rollback))]
    pub camera_pitch: i32,
    /// Rig sensitivity, scaled by 1000.
    #[ara(replicate(audience = Relevant, rollback))]
    pub sensitivity: i32,
    /// Maximum facing change per tick, in millidegrees.
    #[ara(replicate(audience = Relevant, rollback))]
    pub turn_speed: i32,
    /// Jump launch velocity in micrometres per tick.
    #[ara(replicate(audience = Relevant, rollback))]
    pub jump_speed: i32,
    /// Downward acceleration in micrometres per tick squared.
    #[ara(replicate(audience = Relevant, rollback))]
    pub gravity: i32,
    /// Vertical velocity in micrometres per tick.
    #[ara(replicate(audience = Relevant, rollback))]
    pub vertical_speed: i32,
    /// Last authoritative supporting height, in millimetres.
    #[ara(replicate(audience = Relevant, rollback))]
    pub floor: i32,
    /// Whether the character has walkable support.
    #[ara(replicate(audience = Relevant, rollback))]
    pub grounded: bool,
    /// Previous jump hold, part of rollback to prevent repeat jumps.
    #[ara(replicate(audience = Relevant, rollback))]
    pub jump_held: bool,
}

/// Signed shortest angle in millidegrees.
pub(crate) fn turn(from: i32, to: i32) -> i32 {
    (to - from + 180_000).rem_euclid(360_000) - 180_000
}

// Bhaskara's rational sine approximation, scaled by one million. Integer math
// keeps camera-relative movement bit-exact across prediction and rollback.
fn sin(angle: i32) -> i64 {
    let a = angle.rem_euclid(360_000);
    let (a, sign) = if a > 180_000 {
        (a - 180_000, -1)
    } else {
        (a, 1)
    };
    let p = i64::from(a) * i64::from(180_000 - a);
    sign * 16 * p * 1_000_000 / (5 * 180_000_i64.pow(2) - 4 * p)
}

/// The native predicted locomotion feature (version 2 includes orbit/sprint/jump).
pub struct Locomotion;
impl SimulationFeature for Locomotion {
    type State = NetMotor;
    type Input = MotorInput;
    const ACTION: &'static str = MOVE;
    const VERSION: u32 = 3;

    fn input(action: Option<ActionValue>) -> std::result::Result<MotorInput, SimError> {
        let mut actions = BTreeMap::new();
        if let Some(value) = action {
            actions.insert(MOVE.into(), value);
        }
        sample(&actions)
    }
    fn action(input: &MotorInput) -> std::result::Result<ActionValue, SimError> {
        expand(input)?;
        Ok(ActionValue::Axis2(input.movement.map(f32::from)))
    }
    fn step(state: &mut NetMotor, input: &MotorInput) {
        let orbit = |axis: i32| i64::from(axis) * i64::from(state.sensitivity) / 1000;
        state.camera_yaw = turn(
            0,
            state.camera_yaw + orbit(input.look[0]).rem_euclid(360_000) as i32,
        );
        state.camera_pitch =
            (i64::from(state.camera_pitch) + orbit(input.look[1])).clamp(-80_000, 80_000) as i32;
        let [x, z] = input.movement;
        if x != 0 || z != 0 {
            let speed = i64::from(if input.sprint {
                state.run_speed
            } else {
                state.speed
            });
            let speed = if x != 0 && z != 0 {
                speed * 707 / 1000
            } else {
                speed
            };
            let (s, c) = (sin(state.camera_yaw), sin(state.camera_yaw + 90_000));
            state.x += ((s * i64::from(z) - c * i64::from(x)) * speed / 1_000_000) as i32;
            state.z += ((c * i64::from(z) + s * i64::from(x)) * speed / 1_000_000) as i32;
            let relative = match (-x, z) {
                (0, 1) => 0,
                (1, 1) => 45_000,
                (1, 0) => 90_000,
                (1, -1) => 135_000,
                (0, -1) => 180_000,
                (-1, -1) => -135_000,
                (-1, 0) => -90_000,
                _ => -45_000,
            };
            let target = turn(0, state.camera_yaw + relative);
            state.heading = turn(
                0,
                state.heading
                    + turn(state.heading, target).clamp(-state.turn_speed, state.turn_speed),
            );
        }
        if input.jump && !state.jump_held && state.grounded {
            state.vertical_speed = state.jump_speed;
            state.grounded = false;
        }
        state.jump_held = input.jump;
        if !state.grounded {
            state.vertical_speed -= state.gravity;
            state.y += state.vertical_speed / 1000;
            if state.y <= state.floor && state.vertical_speed <= 0 {
                state.y = state.floor;
                state.vertical_speed = 0;
                state.grounded = true;
            }
        }
    }
    fn interpolate(a: NetMotor, b: NetMotor, alpha: f32) -> NetMotor {
        let lerp = |a: i32, b: i32| a + ((b - a) as f32 * alpha).round() as i32;
        NetMotor {
            x: lerp(a.x, b.x),
            y: lerp(a.y, b.y),
            z: lerp(a.z, b.z),
            heading: turn(
                0,
                a.heading + (turn(a.heading, b.heading) as f32 * alpha).round() as i32,
            ),
            ..b
        }
    }
    fn correction(a: &NetMotor, b: &NetMotor) -> f64 {
        f64::from(a.x - b.x)
            .hypot(f64::from(a.y - b.y))
            .hypot(f64::from(a.z - b.z))
    }
    fn position(state: &NetMotor) -> Option<[f32; 2]> {
        Some(state.ground())
    }
}

impl NetMotor {
    /// Starts a motor at the authored pose. The caller supplies its camera rig.
    pub fn at(transform: &Transform, control: &PlayerControl, hz: u32) -> Self {
        let hz = hz.max(1) as f32;
        let mm = |m: f32| (m * 1000.0).round() as i32;
        Self {
            x: mm(transform.position[0]),
            y: mm(transform.position[1]),
            z: mm(transform.position[2]),
            floor: mm(transform.position[1]),
            heading: mm(control.yaw),
            speed: mm(control.walk_speed / hz),
            run_speed: mm(control.run_speed / hz),
            turn_speed: mm(control.turn_rate / hz),
            jump_speed: (control.jump_speed * 1_000_000.0 / hz).round() as i32,
            gravity: (9.81 * 1_000_000.0 / (hz * hz)).round() as i32,
            sensitivity: 1000,
            grounded: true,
            ..Default::default()
        }
    }
    /// Ground-plane position in metres.
    pub fn ground(&self) -> [f32; 2] {
        [self.x as f32 / 1000.0, self.z as f32 / 1000.0]
    }
    /// Character facing in degrees.
    pub fn yaw(&self) -> f32 {
        self.heading as f32 / 1000.0
    }
}

pub(crate) fn sample(
    actions: &BTreeMap<String, ActionValue>,
) -> std::result::Result<MotorInput, SimError> {
    let axis = |name: &str| match actions.get(name) {
        None => Ok([0.0; 2]),
        Some(ActionValue::Axis2(v)) if v.iter().all(|v| v.is_finite()) => Ok(*v),
        _ => Err(SimError(format!("{name} requires a finite Axis2"))),
    };
    let button = |name: &str| match actions.get(name) {
        None => Ok(false),
        Some(ActionValue::Button(v)) => Ok(*v),
        _ => Err(SimError(format!("{name} requires a button"))),
    };
    let movement = axis(MOVE)?.map(|v| v.clamp(-1.0, 1.0).round() as i8);
    let look = axis(LOOK)?;
    if look.iter().any(|v| v.abs() > 180.0) {
        return Err(SimError("Look exceeds 180 degrees per tick".into()));
    }
    Ok(MotorInput {
        movement,
        look: look.map(|v| (v * 1000.0).round() as i32),
        sprint: button(SPRINT)?,
        jump: button(JUMP)?,
        interact: button(INTERACT)?,
        inventory: button(INVENTORY)?,
    })
}

pub(crate) fn expand(
    input: &MotorInput,
) -> std::result::Result<Vec<(String, ActionValue)>, SimError> {
    if input.movement.iter().any(|v| !(-1..=1).contains(v))
        || input.look.iter().any(|v| !(-180_000..=180_000).contains(v))
    {
        return Err(SimError("motor input outside declared bounds".into()));
    }
    Ok(vec![
        (
            MOVE.into(),
            ActionValue::Axis2(input.movement.map(f32::from)),
        ),
        (
            LOOK.into(),
            ActionValue::Axis2(input.look.map(|v| v as f32 / 1000.0)),
        ),
        (SPRINT.into(), ActionValue::Button(input.sprint)),
        (JUMP.into(), ActionValue::Button(input.jump)),
        (INTERACT.into(), ActionValue::Button(input.interact)),
        (INVENTORY.into(), ActionValue::Button(input.inventory)),
    ])
}

pub(crate) fn codec() -> ara_net::ActionCodec<MotorInput> {
    ara_net::ActionCodec {
        names: &[MOVE, LOOK, SPRINT, JUMP, INTERACT, INVENTORY],
        sample,
        expand,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagonals_keep_speed_and_interpolation_takes_the_short_arc() {
        let mut motor = NetMotor {
            speed: 50,
            turn_speed: 9000,
            ..Default::default()
        };
        Locomotion::step(
            &mut motor,
            &MotorInput {
                movement: [0, 1],
                ..Default::default()
            },
        );
        Locomotion::step(
            &mut motor,
            &MotorInput {
                movement: [1, 1],
                ..Default::default()
            },
        );
        assert_eq!((motor.x, motor.z, motor.heading), (-35, 85, -9000));
        let mid = Locomotion::interpolate(
            NetMotor {
                heading: 170_000,
                ..Default::default()
            },
            NetMotor {
                heading: -170_000,
                x: 100,
                ..Default::default()
            },
            0.5,
        );
        assert_eq!((mid.x, mid.heading), (50, -180_000));
        assert_eq!(turn(170_000, -170_000), 20_000);
    }
    #[test]
    fn orbit_sprint_jump_and_rollback_share_one_input() {
        let mut state = NetMotor {
            speed: 50,
            run_speed: 100,
            turn_speed: 9000,
            sensitivity: 1000,
            grounded: true,
            jump_speed: 100_000,
            gravity: 2725,
            ..Default::default()
        };
        let input = MotorInput {
            movement: [0, 1],
            look: [90_000, 0],
            sprint: true,
            jump: true,
        };
        let before = state;
        Locomotion::step(&mut state, &input);
        assert_eq!((state.x, state.z, state.camera_yaw), (100, 0, 90_000));
        assert!(state.y > 0 && !state.grounded);
        let mut replayed = before;
        Locomotion::step(&mut replayed, &input);
        assert_eq!(state, replayed);
        for _ in 0..100 {
            Locomotion::step(
                &mut state,
                &MotorInput {
                    jump: true,
                    ..Default::default()
                },
            );
        }
        assert!(
            state.grounded && state.y == 0,
            "holding jump must not retrigger"
        );
    }
    #[test]
    fn codec_rejects_unbounded_or_wrongly_typed_controls() {
        assert!(
            sample(&BTreeMap::from([(
                LOOK.into(),
                ActionValue::Axis2([f32::NAN, 0.0])
            )]))
            .is_err()
        );
        assert!(
            expand(&MotorInput {
                look: [180_001, 0],
                ..Default::default()
            })
            .is_err()
        );
        assert!(sample(&BTreeMap::from([(SPRINT.into(), ActionValue::Axis(1.0))])).is_err());
    }
}
