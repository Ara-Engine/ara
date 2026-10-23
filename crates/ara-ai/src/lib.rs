//! Gameplay AI for Ara: hierarchical state trees with utility selection, EQS
//! spatial queries, perception-driven conditions and intents.
//!
//! - Author behaviours as text ([`dsl`]) or graphs ([`projection`]); both are one
//!   [`ast::Behavior`] stored as an `ara-graph` document (domain `behavior`),
//!   edited only through typed commands ([`commands`]) with undo.
//! - [`compile`] checks names and types into an immutable [`Program`].
//! - [`register_sim`] installs the fixed-tick systems: perception, brains, the
//!   intent bridge to `ara-nav`, locomotion intents for animation, and `ara why`
//!   status reports ([`why`]).
//! - Brains write intents ([`MoveIntent`], [`LookIntent`], [`ActionIntent`],
//!   [`LocomotionIntent`]); they never call navigation, animation or gameplay.
//! - All gameplay state is in components ([`BrainState`], `NavAgentState`,
//!   `Perception`); runs are deterministic for any worker count.
#![warn(missing_docs)]

pub mod ast;
pub mod commands;
mod components;
pub mod dsl;
mod eval;
mod los;
pub mod overlay;
mod program;
pub mod projection;
mod system;
mod think;
mod trace;
pub mod why;

pub use components::{
    ActionIntent, Brain, BrainState, Change, LocomotionIntent, LookIntent, MoveIntent, TaskState,
    TaskStatus, register,
};
pub use eval::{EYE, EqsItem, FAR, HEARD_WINDOW};
pub use los::NavLineOfSight;
pub use program::{
    Builtin, CAction, CTrigger, ExprId, Generator, Node, Program, Query, Slot, StateNode, Task,
    Value, builtin_names, compile,
};
pub use system::{
    AiConfig, AiWorld, Sight, bridge, bundle, locomotion, perceive, register_sim, sync, think, view,
};
pub use think::HYSTERESIS;
pub use trace::{BrainView, TraceEvent};

/// Diagnostic codes (navigation/AI range 7100-7199; ara-nav owns 7100).
pub mod codes {
    use ara_diag::Code;
    /// The agent cannot reach its move target (no path, off mesh, no mesh).
    pub const NO_PATH: Code = Code(7101);
    /// The agent is blocked: waiting at a closed link, or stalled by obstacles/crowd.
    pub const BLOCKED: Code = Code(7102);
    /// Behaviour text does not parse.
    pub const SYNTAX: Code = Code(7110);
    /// Unknown or duplicate name (variable, state, query, function).
    pub const NAME: Code = Code(7111);
    /// Type mismatch in an expression, action or task argument.
    pub const TYPE: Code = Code(7112);
    /// Unknown transition target, task, generator or bad call arguments.
    pub const TARGET: Code = Code(7113);
    /// A plain-text rename deleted an identity; `behavior.rename` restores it.
    pub const PLAIN_RENAME: Code = Code(7114);
    /// The state tree has been stuck in one state with finished/failed tasks.
    pub const STUCK: Code = Code(7115);
    /// A state waits on perception that never fired (target never seen/heard).
    pub const UNSEEN: Code = Code(7116);
    /// The brain has no (valid) behaviour.
    pub const NO_BEHAVIOR: Code = Code(7117);
}
