use crate::Identity;
use ara_input::{ActionValue, InputState, PlayerId};
use bevy_ecs::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fixed simulation context. The only RNG source available to gameplay systems.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct SimContext {
    /// Completed/current tick, starting at zero before the first system run.
    pub(crate) tick: u64,
    /// Fixed step in seconds; never affected by presentation rate or timescale.
    pub(crate) dt: f64,
    /// Applied input state for this boundary.
    pub(crate) inputs: InputState,
    pub(crate) seed: u64,
    streams: BTreeMap<String, BTreeMap<String, u64>>,
    next_identity: u64,
}

impl SimContext {
    /// Current fixed simulation tick, beginning at zero.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Fixed simulation step in seconds, independent of timescale.
    pub fn dt(&self) -> f64 {
        self.dt
    }
    pub(crate) fn new(seed: u64, hz: u32) -> Self {
        Self {
            tick: 0,
            dt: 1.0 / f64::from(hz),
            inputs: InputState::default(),
            seed,
            streams: BTreeMap::new(),
            next_identity: 0,
        }
    }

    /// Simulation time in seconds; advances only when a fixed tick executes.
    pub fn elapsed(&self) -> f64 {
        self.tick as f64 * self.dt
    }

    /// Active action for an explicit player; absent means neutral.
    pub fn action(&self, player: PlayerId, action: &str) -> Option<ActionValue> {
        self.inputs.get(player, action)
    }

    /// Independent deterministic stream for a stable system name and entity identity.
    /// Borrow once per entity update for several draws; other streams' traversal
    /// order cannot affect its sequence.
    pub fn rng(&mut self, system: &str, entity: &str) -> StreamRng<'_> {
        let seed = self.seed;
        let state = self
            .streams
            .entry(system.into())
            .or_default()
            .entry(entity.into())
            .or_insert_with(|| {
                let mut hasher = blake3::Hasher::new();
                hasher.update(&seed.to_le_bytes());
                hasher.update(&(system.len() as u64).to_le_bytes());
                hasher.update(system.as_bytes());
                hasher.update(entity.as_bytes());
                let mut bytes = [0; 8];
                bytes.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
                u64::from_le_bytes(bytes)
            });
        StreamRng { state }
    }

    /// Allocates a deterministic stable identity for runtime structural commands.
    /// Authoring IDs and these e_rt-prefixed identities have separate lifecycles.
    pub fn allocate_identity(&mut self) -> Identity {
        let id = Identity(format!("e_rt{:016x}", self.next_identity));
        self.next_identity = self
            .next_identity
            .checked_add(1)
            .expect("runtime identity space is exhausted");
        id
    }
}

/// A borrowed SplitMix64 stream; its entire state lives in SimContext snapshots.
pub struct StreamRng<'a> {
    state: &'a mut u64,
}

impl StreamRng<'_> {
    /// Next deterministic integer.
    pub fn next_u64(&mut self) -> u64 {
        *self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A uniform float in [0, 1), using 24 significant bits.
    pub fn unit_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 16_777_216.0
    }
}
