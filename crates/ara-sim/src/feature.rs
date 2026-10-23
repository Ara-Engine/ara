//! Replayable feature contract shared by Local, authority and predicted owners.
use crate::SimError;
use ara_input::{ActionValue, PlayerId};
use ara_schema::Schema;
use bevy_ecs::{component::Mutable, prelude::Component};
use serde::{Serialize, de::DeserializeOwned};

/// One fixed-step feature with explicit, complete restorable state. Dependencies
/// affecting its next step must be included in `State`; presentation stays outside.
pub trait SimulationFeature: Send + Sync + 'static {
    /// Complete state/history unit stored in the ECS.
    type State: Component<Mutability = Mutable>
        + Schema
        + Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + std::fmt::Debug;
    /// Bounded tick input, independent of devices and networking libraries.
    type Input: Serialize + DeserializeOwned + Clone + Default + PartialEq + std::fmt::Debug;
    /// Action name sampled by devices, bots and network adapters.
    const ACTION: &'static str;
    /// Compatibility version of the input/step contract; bump on incompatible changes.
    const VERSION: u32 = 1;
    /// Converts a declared action to typed input and validates numeric bounds.
    fn input(action: Option<ActionValue>) -> Result<Self::Input, SimError>;
    /// Converts typed input back to the shared action boundary.
    fn action(input: &Self::Input) -> Result<ActionValue, SimError>;
    /// Runs exactly once per simulation tick (or once per resimulated tick).
    fn step(state: &mut Self::State, input: &Self::Input);
    /// Samples a remote view without mutating authoritative state.
    fn interpolate(a: Self::State, b: Self::State, alpha: f32) -> Self::State;
    /// Diagnostic correction magnitude in the feature's documented units.
    fn correction(a: &Self::State, b: &Self::State) -> f64;
    /// Optional world-space interest position in metres, derived from complete state.
    /// Features without spatial state leave this to the authority's scene mapping.
    fn position(state: &Self::State) -> Option<[f32; 2]> {
        let _ = state;
        None
    }
}

/// Replay-safe predicted effect identity; reconnects must retain the player's epoch.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct PredictionKey {
    /// Authenticated input owner.
    pub player: PlayerId,
    /// Simulation tick at which the command was first sampled.
    pub tick: u64,
    /// Monotonic sequence within the player's session.
    pub sequence: u64,
}
