//! The command registry (decision A4): every authoring mutation is a typed command.
//!
//! - Define a [`Command`], [`Query`] or [`Job`] with a params type that derives
//!   `Schema` (so the params have a JSON Schema) and `Deserialize`, then register it.
//!   The registry's [`catalog`](Registry::catalog) feeds `reflection.json`, the CLI,
//!   the palette and MCP.
//! - `run(ctx, params)` changes documents only through [`EditCtx`], so every change is
//!   an op in a transaction: undo, review and the op stream come for free.
//! - [`Registry::apply`] runs a [`Batch`] of calls as one transaction. Calls can mint
//!   symbolic IDs (`"id": "$crate"`) that later calls use; the result maps them to
//!   real IDs. `dryRun` returns the diff and the same errors without applying.
//! - Entity params take IDs, addresses, symbols or name selectors (`name:Crate`,
//!   `name:Floor/Crate`); [`select`] resolves selectors before validation.
//! - Errors are [`Diagnostic`](ara_diag::Diagnostic)s with stable codes, the call
//!   index and the params path.
//!
//! [`builtin`] holds the starter set (entities, components, properties, transforms,
//! prefabs).

extern crate glam34 as glam;

pub mod builtin;
mod ctx;
mod registry;
pub mod select;

pub use ctx::{EditCtx, JobCtx, ReadCtx, Target};
pub use registry::{
    Batch, BatchResult, Call, Command, DocumentTemplate, Hints, Info, Job, Kind, Query, Registry,
    Session,
};
