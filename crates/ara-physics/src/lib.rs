//! Ara-owned physics contract, schemas and fixed-tick ECS seam.
//!
//! Authoring uses reflected components and ara-ops transactions. Runtime intents
//! are applied in stable identity order at a fixed tick; they never mutate docs.
//! `register` installs the headless Bevy 0.20 integration without an ara-sim dependency.
//! `PhysicsSnapshot` is paired with the simulation's ECS/RNG/input snapshot.

mod api;
mod backend;
mod components;
mod debug;
mod ecs;
mod events;
mod query;
mod state;
mod world;

pub use api::*;
/// Engine quaternion value, shared with the authoring schema.
pub use ara_schema::Quat as Rotation;
pub use backend::*;
pub use components::*;
pub use debug::*;
pub use ecs::*;
pub use events::*;
pub use query::*;
pub use state::*;
pub use world::*;
