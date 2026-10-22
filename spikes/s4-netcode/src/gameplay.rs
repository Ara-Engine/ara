//! The complete simulation feature: ECS state, typed input and one step function.
//! This module has no networking backend imports or network resources.

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::{IntentSchema, component, intent};

pub const PLAYERS: usize = 4;
pub const ENTITIES: usize = 50;
pub const HZ: u32 = 60;

component! {
    #[net(1, Relevant, Millimetres, Linear, true)]
    /// Complete restorable motor state, integer millimetres at 60 Hz.
    pub struct Motor {
        /// Horizontal position in millimetres.
        pub x: i32,
        /// Second horizontal axis in millimetres.
        pub z: i32,
    }
}
component! {
    #[net(2, Relevant, Exact, None, false)]
    /// Durable shared door state, including the authoritative change tick.
    pub struct Door {
        /// Whether traversal is permitted.
        pub open: bool,
        /// Last successful mutation tick.
        pub changed: u32,
    }
}
component! {
    #[net(3, OwnerOnly, Exact, None, false)]
    /// Private inventory; one bit per pickup, bounded to 45 items.
    pub struct Inventory {
        /// Input owner whose connection may receive this value.
        pub owner: u8,
        /// Collected item identities.
        pub items: u64,
    }
}
component! {
    #[net(4, Relevant, Exact, None, false)]
    /// Shared pickup existence and fixed interaction position.
    pub struct Item {
        /// Whether it is still available.
        pub available: bool,
        /// Pickup coordinate in millimetres.
        pub x: i32,
    }
}
intent! {
    #[intent(10, rate = 5)]
    /// Ask the authority to toggle the shared door.
    pub struct ToggleDoor {
        /// Stable target identity; only 4 is the door in this scene.
        pub target: u16,
    }
}
intent! {
    #[intent(11, rate = 10)]
    /// Ask the authority to acquire an available nearby pickup.
    pub struct PickUp {
        /// Stable item identity in the range 5..50.
        pub target: u16,
    }
}

/// Typed, bounded motor input. Invalid axes are rejected by the authority.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MoveInput {
    /// Signed axis in -1..=1.
    pub x: i8,
    /// Signed axis in -1..=1.
    pub z: i8,
}

/// Discrete requests retain their own rate and target schema.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Intent {
    /// Shared interaction.
    Toggle(ToggleDoor),
    /// Private acquisition.
    PickUp(PickUp),
}

/// Caller is assigned by the session, never trusted from a packet body.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Command {
    /// Authenticated input owner.
    pub caller: u8,
    /// Caller-monotonic idempotency identity.
    pub sequence: u32,
    /// Typed intent payload.
    pub intent: Intent,
}

/// The actual inputs and commands applied at one authoritative tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    /// Absolute simulation tick, starting at one.
    pub tick: u32,
    /// Input for each player; missing network input is neutral.
    pub inputs: [MoveInput; PLAYERS],
    /// Commands in caller/sequence order.
    pub commands: Vec<Command>,
}

#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct CallerState {
    sequence: u32,
    last_toggle: Option<u32>,
    last_pickup: Option<u32>,
}

#[derive(Component)]
struct Clock(u32);

/// Owned ECS world plus stable entity mapping. All mutable gameplay state is ECS data.
pub struct Simulation {
    pub world: World,
    entities: [Entity; ENTITIES],
    clock: Entity,
}

/// Complete authoritative checkpoint, including state that is never replicated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    /// Tick represented by this checkpoint.
    pub tick: u32,
    /// All player motor components in stable identity order.
    pub motors: [Motor; PLAYERS],
    /// Durable shared door state.
    pub door: Door,
    /// Private server state, never placed in a public snapshot.
    pub inventories: [Inventory; PLAYERS],
    /// Pickup lifecycle in stable identity order.
    pub items: Vec<Item>,
    callers: [CallerState; PLAYERS],
}

/// Tick-boundary validation results, recorded as diagnostics rather than game state.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Outcomes {
    pub accepted: u32,
    pub rejected: u32,
}

/// The identical motor function used by offline, authority, prediction and replay.
pub fn step_motor(state: &mut Motor, input: MoveInput) {
    state.x += i32::from(input.x) * 25;
    state.z += i32::from(input.z) * 25;
}

impl Simulation {
    /// Construct one headless room with four players, one door and 45 pickups.
    pub fn new() -> Self {
        let mut world = World::new();
        let entities = std::array::from_fn(|id| {
            if id < PLAYERS {
                world
                    .spawn((
                        Motor::default(),
                        Inventory {
                            owner: id as u8,
                            items: 0,
                        },
                        CallerState::default(),
                    ))
                    .id()
            } else if id == PLAYERS {
                world.spawn(Door::default()).id()
            } else {
                world
                    .spawn(Item {
                        available: true,
                        x: 0,
                    })
                    .id()
            }
        });
        let clock = world.spawn(Clock(0)).id();
        Self {
            world,
            entities,
            clock,
        }
    }

    pub fn tick(&self) -> u32 {
        self.world
            .get::<Clock>(self.clock)
            .expect("clock entity remains alive")
            .0
    }

    pub fn motor(&self, player: usize) -> Motor {
        *self
            .world
            .get::<Motor>(self.entities[player])
            .expect("player has motor")
    }

    /// Apply one fixed tick. Arrival queues must already have been drained by the session.
    pub fn step(&mut self, frame: &Frame) -> Outcomes {
        assert_eq!(
            frame.tick,
            self.tick() + 1,
            "simulation ticks must be contiguous"
        );
        for (player, input) in frame.inputs.iter().enumerate() {
            if (-1..=1).contains(&input.x) && (-1..=1).contains(&input.z) {
                let mut motor = self
                    .world
                    .get_mut::<Motor>(self.entities[player])
                    .expect("player has motor");
                step_motor(&mut motor, *input);
            }
        }
        let mut outcomes = Outcomes::default();
        for command in &frame.commands {
            if self.execute(frame.tick, *command) {
                outcomes.accepted += 1;
            } else {
                outcomes.rejected += 1;
            }
        }
        self.world
            .get_mut::<Clock>(self.clock)
            .expect("clock remains alive")
            .0 = frame.tick;
        outcomes
    }

    /// Validate ownership, sequence, range and per-intent cooldown before mutating.
    fn execute(&mut self, tick: u32, command: Command) -> bool {
        let caller = usize::from(command.caller);
        if caller >= PLAYERS {
            return false;
        }
        let player = self.entities[caller];
        let mut state = *self
            .world
            .get::<CallerState>(player)
            .expect("caller state exists");
        if command.sequence <= state.sequence {
            return false;
        }
        state.sequence = command.sequence;
        // Even rejected commands consume their identity; retries cannot bypass validation.
        *self
            .world
            .get_mut::<CallerState>(player)
            .expect("caller state exists") = state;
        match command.intent {
            Intent::Toggle(toggle) => {
                if toggle.target != 4
                    || self.motor(caller).x.abs() > 2_000
                    || state.last_toggle.is_some_and(|last| {
                        tick - last < HZ / u32::from(ToggleDoor::RATE_PER_SECOND)
                    })
                {
                    return false;
                }
                let mut door = self
                    .world
                    .get_mut::<Door>(self.entities[4])
                    .expect("door exists");
                door.open = !door.open;
                door.changed = tick;
                self.world
                    .get_mut::<CallerState>(player)
                    .expect("caller state exists")
                    .last_toggle = Some(tick);
                true
            }
            Intent::PickUp(pickup) => {
                let id = usize::from(pickup.target);
                if !(5..ENTITIES).contains(&id)
                    || state
                        .last_pickup
                        .is_some_and(|last| tick - last < HZ / u32::from(PickUp::RATE_PER_SECOND))
                {
                    return false;
                }
                let item = *self
                    .world
                    .get::<Item>(self.entities[id])
                    .expect("item exists");
                if !item.available || (self.motor(caller).x - item.x).abs() > 2_000 {
                    return false;
                }
                self.world
                    .get_mut::<Item>(self.entities[id])
                    .expect("item exists")
                    .available = false;
                self.world
                    .get_mut::<Inventory>(player)
                    .expect("inventory exists")
                    .items |= 1 << (id - 5);
                self.world
                    .get_mut::<CallerState>(player)
                    .expect("caller state exists")
                    .last_pickup = Some(tick);
                true
            }
        }
    }

    /// Canonical stable-order extraction for replay, hashes and adapter projections.
    pub fn capture(&self) -> State {
        State {
            tick: self.tick(),
            motors: std::array::from_fn(|id| self.motor(id)),
            door: *self
                .world
                .get::<Door>(self.entities[4])
                .expect("door exists"),
            inventories: std::array::from_fn(|id| {
                *self
                    .world
                    .get::<Inventory>(self.entities[id])
                    .expect("inventory exists")
            }),
            items: self.entities[5..]
                .iter()
                .map(|entity| *self.world.get::<Item>(*entity).expect("item exists"))
                .collect(),
            callers: std::array::from_fn(|id| {
                *self
                    .world
                    .get::<CallerState>(self.entities[id])
                    .expect("caller state exists")
            }),
        }
    }
}

/// Repeatable script: move out and back, open a door, reject spam, collect one item each.
pub fn scripted_frame(tick: u32, players: usize) -> Frame {
    let mut frame = Frame {
        tick,
        inputs: [MoveInput::default(); PLAYERS],
        commands: Vec::new(),
    };
    for player in 0..players {
        frame.inputs[player].x = match (player, tick) {
            (0..=2, 100..=159) | (_, 300..=359) => 1,
            (0..=2, 160..=219) | (_, 360..=419) => -1,
            _ => 0,
        };
        let intent = match (player, tick) {
            (0, 40 | 41) => Some(Intent::Toggle(ToggleDoor { target: 4 })),
            (0..=2, 60) | (3, 260) => Some(Intent::PickUp(PickUp {
                target: 5 + player as u16,
            })),
            _ => None,
        };
        if let Some(intent) = intent {
            frame.commands.push(Command {
                caller: player as u8,
                sequence: tick,
                intent,
            });
        }
    }
    frame
}

/// Stable same-machine hash without a serializer or randomized hash iteration.
pub fn hash(state: &State) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut add = |value: u64| {
        for byte in value.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    };
    add(u64::from(state.tick));
    for motor in state.motors {
        add(motor.x as u64);
        add(motor.z as u64);
    }
    add(u64::from(state.door.open));
    add(u64::from(state.door.changed));
    for inventory in state.inventories {
        add(u64::from(inventory.owner));
        add(inventory.items);
    }
    for item in &state.items {
        add(u64::from(item.available));
        add(item.x as u64);
    }
    for caller in state.callers {
        add(u64::from(caller.sequence));
        add(u64::from(caller.last_toggle.unwrap_or(0)));
        add(u64::from(caller.last_pickup.unwrap_or(0)));
    }
    hash
}

/// First differing gameplay field at the authoritative tick, for agent diagnostics.
#[cfg(any(feature = "network", test))]
pub fn first_difference(expected: &State, actual: &State) -> Option<String> {
    if expected.tick != actual.tick {
        return Some(format!("tick: {} != {}", expected.tick, actual.tick));
    }
    for id in 0..PLAYERS {
        if expected.motors[id].x != actual.motors[id].x {
            return Some(format!(
                "tick {} player {id}.motor.x: {} != {}",
                expected.tick, expected.motors[id].x, actual.motors[id].x
            ));
        }
        if expected.motors[id].z != actual.motors[id].z {
            return Some(format!("tick {} player {id}.motor.z", expected.tick));
        }
        if expected.inventories[id].owner != actual.inventories[id].owner {
            return Some(format!(
                "tick {} player {id}.inventory.owner",
                expected.tick
            ));
        }
        if expected.inventories[id].items != actual.inventories[id].items {
            return Some(format!(
                "tick {} player {id}.inventory.items",
                expected.tick
            ));
        }
        if expected.callers[id].sequence != actual.callers[id].sequence {
            return Some(format!(
                "tick {} player {id}.intent_history.sequence",
                expected.tick
            ));
        }
        if expected.callers[id].last_toggle != actual.callers[id].last_toggle {
            return Some(format!(
                "tick {} player {id}.intent_history.last_toggle",
                expected.tick
            ));
        }
        if expected.callers[id].last_pickup != actual.callers[id].last_pickup {
            return Some(format!(
                "tick {} player {id}.intent_history.last_pickup",
                expected.tick
            ));
        }
    }
    if expected.door.open != actual.door.open {
        return Some(format!("tick {} door.open", expected.tick));
    }
    if expected.door.changed != actual.door.changed {
        return Some(format!("tick {} door.changed", expected.tick));
    }
    for (id, (a, b)) in expected.items.iter().zip(&actual.items).enumerate() {
        if a.available != b.available {
            return Some(format!("tick {} item {}.available", expected.tick, id + 5));
        }
        if a.x != b.x {
            return Some(format!("tick {} item {}.x", expected.tick, id + 5));
        }
    }
    if expected.items.len() != actual.items.len() {
        return Some(format!("tick {} item_count", expected.tick));
    }
    None
}
