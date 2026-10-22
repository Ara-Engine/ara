//! Renet fallback: schema registration, bounded codecs, visibility and prediction.
//! Lightyear/Replicon failed the pinned toolchain probe; see compatibility/.

use crate::{
    gameplay::{self, Door, Inventory, Item, Motor, MoveInput, PickUp, State, ToggleDoor},
    schema::{Audience, IntentSchema, Interpolation, NetSchema, Policy, Quantization},
};
use bevy_ecs::prelude::{Entity, World};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_MESSAGE: usize = 16_384;

/// Full visible baseline. There is no field capable of containing all inventories.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub tick: u32,
    pub motors: [Motor; 4],
    pub door: Door,
    pub items: Vec<Item>,
    pub inventory: Inventory,
}

/// Adapter compilation consumes the policies once; gameplay knows no backend types.
pub struct Registry {
    policies: Vec<Policy>,
    intents: Vec<(u16, &'static str, u16)>,
    pub bytes_encoded: u64,
}

impl Registry {
    pub fn new() -> Self {
        let mut result = Self {
            policies: Vec::new(),
            intents: Vec::new(),
            bytes_encoded: 0,
        };
        result.register::<Motor>();
        result.register::<Door>();
        result.register::<Inventory>();
        result.register::<Item>();
        result.register_intent::<ToggleDoor>();
        result.register_intent::<PickUp>();
        result
    }

    fn register<T: NetSchema>(&mut self) {
        assert!(
            !self.policies.iter().any(|policy| policy.id == T::POLICY.id),
            "schema IDs must be unique"
        );
        self.policies.push(T::POLICY);
    }

    fn register_intent<T: IntentSchema>(&mut self) {
        assert!(
            !self.policies.iter().any(|policy| policy.id == T::ID)
                && !self.intents.iter().any(|(id, _, _)| *id == T::ID),
            "intent schema IDs must be unique"
        );
        self.intents.push((T::ID, T::NAME, T::RATE_PER_SECOND));
    }

    /// Validate this small backend's supported projection, rather than ignoring metadata.
    pub fn validate(&self) -> Result<(), String> {
        for (_, name, rate) in &self.intents {
            if *rate == 0 || u32::from(*rate) > gameplay::HZ {
                return Err(format!("unsupported rate for {name}"));
            }
        }
        for policy in &self.policies {
            let supported = match policy.name {
                "Motor" => {
                    policy.audience == Audience::Relevant
                        && policy.quantization == Quantization::Millimetres
                        && policy.interpolation == Interpolation::Linear
                        && policy.rollback
                }
                "Inventory" => {
                    policy.audience == Audience::OwnerOnly
                        && policy.quantization == Quantization::Exact
                        && !policy.rollback
                }
                "Door" | "Item" => {
                    policy.audience == Audience::Relevant
                        && policy.quantization == Quantization::Exact
                        && policy.interpolation == Interpolation::None
                        && !policy.rollback
                }
                _ => false,
            };
            if !supported {
                return Err(format!("unsupported schema policy for {}", policy.name));
            }
        }
        Ok(())
    }

    /// Filter before serialization, including baselines for late joiners.
    pub fn project(&self, state: &State, recipient: usize) -> Snapshot {
        assert!(recipient < 4, "recipient was authenticated and bounded");
        Snapshot {
            tick: state.tick,
            motors: state.motors,
            door: state.door,
            items: state.items.to_vec(),
            inventory: state.inventories[recipient],
        }
    }

    pub fn encode<T: Serialize>(&mut self, value: &T) -> Result<Vec<u8>, String> {
        let bytes = postcard::to_allocvec(value).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_MESSAGE {
            return Err("encoded message exceeds bound".into());
        }
        self.bytes_encoded += bytes.len() as u64;
        Ok(bytes)
    }
}

pub fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, String> {
    if bytes.len() > MAX_MESSAGE {
        return Err("received message exceeds bound".into());
    }
    postcard::from_bytes(bytes).map_err(|error| error.to_string())
}

/// Restore a checkpoint, then run precisely the same step over retained owner inputs.
/// Histories belong to this network-only adapter, never to LocalSession.
pub struct Prediction {
    world: World,
    entity: Entity,
    pub tick: u32,
    inputs: BTreeMap<u32, MoveInput>,
    history: BTreeMap<u32, Motor>,
    pub corrections_mm: Vec<f64>,
    pub checked: u32,
    pub resimulated_ticks: u32,
}

impl Prediction {
    pub fn new() -> Self {
        let mut world = World::new();
        let entity = world.spawn(Motor::default()).id();
        Self {
            world,
            entity,
            tick: 0,
            inputs: BTreeMap::new(),
            history: BTreeMap::from([(0, Motor::default())]),
            corrections_mm: Vec::new(),
            checked: 0,
            resimulated_ticks: 0,
        }
    }

    pub fn motor(&self) -> Motor {
        *self
            .world
            .get::<Motor>(self.entity)
            .expect("predicted motor component exists")
    }

    pub fn advance(&mut self, tick: u32, input: MoveInput) {
        assert_eq!(tick, self.tick + 1, "prediction advances contiguous ticks");
        gameplay::step_motor(
            &mut self
                .world
                .get_mut::<Motor>(self.entity)
                .expect("predicted motor component exists"),
            input,
        );
        self.tick = tick;
        self.inputs.insert(tick, input);
        self.history.insert(tick, self.motor());
    }

    pub fn reconcile(&mut self, tick: u32, motor: Motor) {
        if let Some(predicted) = self.history.get(&tick) {
            self.checked += 1;
            let dx = f64::from(motor.x - predicted.x);
            let dz = f64::from(motor.z - predicted.z);
            let magnitude = dx.hypot(dz);
            if magnitude > 0.0 {
                self.corrections_mm.push(magnitude);
            }
        }
        if tick <= self.tick {
            let mut restored = motor;
            self.history.insert(tick, motor);
            for replay_tick in tick + 1..=self.tick {
                let input = self.inputs.get(&replay_tick).copied().unwrap_or_default();
                gameplay::step_motor(&mut restored, input);
                self.history.insert(replay_tick, restored);
                self.resimulated_ticks += 1;
            }
            *self
                .world
                .get_mut::<Motor>(self.entity)
                .expect("predicted motor component exists") = restored;
        } else {
            *self
                .world
                .get_mut::<Motor>(self.entity)
                .expect("predicted motor component exists") = motor;
            self.tick = tick;
            self.history.insert(tick, motor);
        }
        self.inputs.retain(|&t, _| t > tick);
        self.history.retain(|&t, _| t >= tick);
    }
}

/// Network-only presentation checkpoints, sampled without touching gameplay state.
pub struct RemoteView {
    older: Option<Snapshot>,
    newer: Option<Snapshot>,
}

impl RemoteView {
    pub fn new() -> Self {
        Self {
            older: None,
            newer: None,
        }
    }
    pub fn push(&mut self, snapshot: Snapshot) {
        self.older = self.newer.take();
        self.newer = Some(snapshot);
    }
    pub fn sample_x(&self, player: usize, tick: f64) -> Option<f64> {
        let newer = self.newer.as_ref()?;
        let Some(older) = self.older.as_ref() else {
            return Some(f64::from(newer.motors[player].x));
        };
        let span = newer.tick.saturating_sub(older.tick);
        let alpha = if span == 0 {
            1.0
        } else {
            ((tick - f64::from(older.tick)) / f64::from(span)).clamp(0.0, 1.0)
        };
        Some(
            f64::from(older.motors[player].x)
                + f64::from(newer.motors[player].x - older.motors[player].x) * alpha,
        )
    }
}

/// Compare authoritative projections at one tick, never private server data to a client.
pub fn first_difference(expected: &Snapshot, actual: &Snapshot) -> Option<String> {
    if expected.tick != actual.tick {
        return Some(format!("tick {} != {}", expected.tick, actual.tick));
    }
    for id in 0..4 {
        if expected.motors[id].x != actual.motors[id].x {
            return Some(format!(
                "tick {} player {id}.motor.x: {} != {}",
                expected.tick, expected.motors[id].x, actual.motors[id].x
            ));
        }
        if expected.motors[id].z != actual.motors[id].z {
            return Some(format!("tick {} player {id}.motor.z", expected.tick));
        }
    }
    if expected.door.open != actual.door.open {
        return Some(format!("tick {} door.open", expected.tick));
    }
    if expected.door.changed != actual.door.changed {
        return Some(format!("tick {} door.changed", expected.tick));
    }
    if expected.inventory.owner != actual.inventory.owner {
        return Some(format!("tick {} inventory.owner", expected.tick));
    }
    if expected.inventory.items != actual.inventory.items {
        return Some(format!("tick {} inventory.items", expected.tick));
    }
    if expected.items.len() != actual.items.len() {
        return Some(format!("tick {} item_count", expected.tick));
    }
    for (id, (a, b)) in expected.items.iter().zip(&actual.items).enumerate() {
        if a.available != b.available {
            return Some(format!("tick {} item {}.available", expected.tick, id + 5));
        }
        if a.x != b.x {
            return Some(format!("tick {} item {}.x", expected.tick, id + 5));
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn visibility_is_enforced_before_encoding_and_after_decoding() {
        let mut registry = Registry::new();
        registry.validate().unwrap();
        let mut state = gameplay::Simulation::new().capture();
        for (id, inventory) in state.inventories.iter_mut().enumerate() {
            inventory.items = 0x1234_5678_0000 + id as u64;
        }
        for recipient in 0..4 {
            let encoded = registry
                .encode(&registry.project(&state, recipient))
                .unwrap();
            let decoded: Snapshot = decode(&encoded).unwrap();
            assert_eq!(decoded.inventory, state.inventories[recipient]);
            assert_eq!(decoded.items.len(), 45);
            for other in 0..4 {
                if other != recipient {
                    assert_ne!(decoded.inventory, state.inventories[other]);
                }
            }
        }
    }

    #[test]
    fn correction_restores_and_replays_the_complete_motor_state() {
        let mut prediction = Prediction::new();
        for tick in 1..=10 {
            prediction.advance(tick, MoveInput { x: 1, z: -1 });
        }
        prediction.reconcile(5, Motor { x: 100, z: -100 });
        assert_eq!(prediction.motor(), Motor { x: 225, z: -225 });
        assert_eq!(prediction.corrections_mm.len(), 1);
        assert_eq!(prediction.resimulated_ticks, 5);
    }

    #[test]
    fn remote_interpolation_is_presentation_only() {
        let registry = Registry::new();
        let mut simulation = gameplay::Simulation::new();
        let mut view = RemoteView::new();
        let a = registry.project(&simulation.capture(), 0);
        view.push(a);
        simulation.step(&gameplay::Frame {
            tick: 1,
            inputs: [MoveInput { x: 1, z: 0 }; 4],
            commands: Vec::new(),
        });
        view.push(registry.project(&simulation.capture(), 0));
        assert_eq!(view.sample_x(1, 0.5), Some(12.5));
        assert_eq!(simulation.motor(1).x, 25);
    }
}
