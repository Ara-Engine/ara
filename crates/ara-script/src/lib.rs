//! The C# host (decisions D11, A13): C# gameplay code on the Rust engine.
//!
//! - [`abi`]: the one generated, layout-hashed `#[repr(C)]` ABI
//!   (`HostInit` + `HostApi` + `ScriptApi`).
//! - [`csgen`]: C# bindings generated from the ABI and the Schema DB.
//! - [`ScriptHost`]: the runtime attached to a bevy `World`: C# components registered
//!   back into the Schema DB, systems scheduled from their read/write sets, behaviours,
//!   commands as undoable steps, and hot reload at tick boundaries.
//! - [`run`]: the `ara_run` supervisor: agent scripts in a limited worker process.
//! - [`codes`]: the host's diagnostic codes (`ARA0900`–`ARA0999`).

pub mod abi;
mod boost;
mod clr;
pub mod codes;
pub mod csgen;
mod ecs;
mod host;
mod module;
mod reload_diag;
pub mod run;
pub mod shipping;
pub mod value;

pub use boost::register_tick_thread;
pub use clr::Runtime;
pub use ecs::cmdbuf::Applied;
pub use host::{
    Boundary, CommandRecord, ComponentWrite, Cue, EditOrigin, EngineBindings, LogSink,
    ModuleStatus, ResourceWrite, ScriptConfig, ScriptHost,
};
pub use module::{MODULE_FILE, ModuleProfile, ModuleSpec, load_order};

/// Engine columns exposed by the standard C# runtime, in ABI binding order.
/// Schemas alone do not expose Rust storage: additions need an explicit binding
/// whose size, alignment and field offsets are checked by the host.
pub fn engine_bindings() -> EngineBindings {
    bind_component!(
        EngineBindings::new(),
        ara_components::Transform as "Transform" { position, rotation, scale }
    )
}
