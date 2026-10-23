//! The C# host (decisions D11, A13): C# gameplay code on the Rust engine.
//!
//! - [`abi`]: the one generated, layout-hashed `#[repr(C)]` ABI
//!   (`HostInit` + `HostApi` + `ScriptApi`).
//! - [`csgen`]: C# bindings generated from the ABI and the Schema DB.
//! - [`ScriptHost`]: the runtime attached to a bevy `World`: C# components registered
//!   back into the Schema DB, systems scheduled from their read/write sets, behaviours,
//!   commands as undoable steps, and hot reload at tick boundaries.
//! - [`codes`]: the host's diagnostic codes (`ARA0900`–`ARA0999`).

pub mod abi;
mod boost;
mod clr;
pub mod codes;
pub mod csgen;
mod ecs;
mod host;
mod module;
pub mod value;

pub use boost::register_tick_thread;
pub use clr::Runtime;
pub use ecs::cmdbuf::Applied;
pub use host::{
    Boundary, CommandRecord, ComponentWrite, Cue, EngineBindings, LogSink, ModuleStatus, ScriptConfig, ScriptHost,
};
pub use module::{MODULE_FILE, ModuleSpec};
