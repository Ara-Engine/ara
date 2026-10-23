//! Typed tick-scoped effects. Replayed ticks replace their effects, never duplicate them.
use crate::{BodyHandle, CharacterHandle};
use bevy_ecs::prelude::Resource;

/// Contact lifecycle for a body/subshape pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContactPhase {
    /// First contact this simulation tick.
    Started,
    /// Continuing contact, useful for grounding/debug views.
    Persisted,
    /// Contact ended, including deactivation or removal.
    Stopped,
}

/// Solid contact, sorted by body IDs, subshape IDs and phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactEvent {
    /// Tick producing the event (completed tick number).
    pub tick: u64,
    /// First body in canonical order.
    pub body_a: BodyHandle,
    /// Second body.
    pub body_b: BodyHandle,
    /// First body's stable application identity, retained for stopped effects after removal.
    pub user_data_a: u64,
    /// Second body's stable application identity.
    pub user_data_b: u64,
    /// First leaf identity.
    pub subshape_a: u32,
    /// Second leaf identity.
    pub subshape_b: u32,
    /// Contact lifecycle.
    pub phase: ContactPhase,
    /// World-space surface point; stopped events carry the last known point.
    pub point: [f32; 3],
    /// Normal from A towards B.
    pub normal: [f32; 3],
}

/// Trigger contact with no solver response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerEvent {
    /// The same ordered contact payload as solid collisions.
    pub contact: ContactEvent,
}

/// The other participant in a virtual character contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysicsActor {
    /// Native rigid body, including sensors and vehicle chassis.
    Body(BodyHandle),
    /// Another virtual character.
    Character(CharacterHandle),
}

/// Restorable character effect; derived from native active contacts, never callback order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterContactEvent {
    /// Completed tick producing this effect.
    pub tick: u64,
    /// Moving character.
    pub character: CharacterHandle,
    /// Character's stable application identity, retained after removal.
    pub user_data: u64,
    /// Body or character touched.
    pub other: PhysicsActor,
    /// Other actor's stable application identity.
    pub other_user_data: u64,
    /// Leaf identity on the other actor.
    pub subshape: u32,
    /// Contact lifecycle.
    pub phase: ContactPhase,
    /// True for a trigger contact; there is no collision response.
    pub sensor: bool,
    /// World-space contact point.
    pub point: [f32; 3],
    /// Normal from this character towards the other actor.
    pub normal: [f32; 3],
}

/// Per-tick event stream. Consumers reconcile by tick after a rollback.
#[derive(Debug, Default, Clone, PartialEq, Resource)]
pub struct PhysicsEvents {
    /// Solid contact lifecycle events.
    pub contacts: Vec<ContactEvent>,
    /// Trigger lifecycle events.
    pub triggers: Vec<TriggerEvent>,
    /// Character/body, character/trigger and character/character contact lifecycles.
    pub character_contacts: Vec<CharacterContactEvent>,
}
