//! Deterministic AI perception: sight cones with pluggable line of sight,
//! hearing from one-tick noise events, and decaying memory.
//!
//! Authoring: [`Perceiver`] (senses) and [`Stimulus`] (seeable things). Runtime:
//! [`PerceptionPose`] (stable ID + pose, synced by the host or `ara-ai`) and
//! [`Perception`] (memory and latest sight checks, complete snapshot state).
//! Gameplay pushes [`Noise`]s into [`NoiseEvents`] before [`sense`] runs.
//! Line of sight comes from a [`LineOfSight`]: physics rays
//! ([`PhysicsLineOfSight`]), navmesh rays (in `ara-ai`) or a closure.
#![warn(missing_docs)]

mod components;
mod los;
mod sense;

pub use components::{
    Known, Noise, NoiseEvents, Perceiver, Perception, PerceptionPose, Sense, SightOutcome,
    Stimulus, kind_hash, register,
};
pub use los::{LineOfSight, OpenSpace, PhysicsLineOfSight};
pub use sense::{PerceptionConfig, PerceptionLos, sense, tick};
