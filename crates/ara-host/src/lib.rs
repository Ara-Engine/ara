//! The Ara project host: one project's documents, history and sessions behind the
//! Engine Protocol (decisions A4–A6, R03 §4.1–4.10).
//!
//! - [`Host::open`] loads a [`Project`], takes its lock and replays `.ara/journal`.
//! - [`Host::call`] (or [`Endpoint`](ara_proto::Endpoint)) runs a method: session and
//!   scope checks and params validation happen here, before any service runs.
//! - [`Service`] is how subsystems (simulation, capture, play, the C# worker) add
//!   methods and MCP tools; the core methods are the `core` service.
//! - Agent transactions are tracked for review (`applied` → `accepted`/`reverted`);
//!   agent turns get automatic checkpoints.
//! - Documents are always saved: debounced canonical flushes. External edits are
//!   merged in and reported, never silently overwritten.
//! - [`docs`] generates `llms.txt` and per-type pages from the registries.

mod core;
pub mod diag;
pub mod docs;
mod files;
mod host;
pub mod jobs;
pub mod journal;
pub mod params;
mod profile;
pub mod project;
pub mod query;
pub mod review;
mod service;
pub mod session;
pub mod text;
pub mod tree;
pub mod why;

pub use host::{Host, HostConfig, RESERVED};
pub use project::Project;
pub use service::{Cx, Method, Service, parse};
pub use session::{Approval, ApprovalRequest, Decision, DefaultPolicy};

/// The method catalog of the services every host registers (core, profile, diag,
/// why), without opening a host (offline docs generation, MCP tests).
pub fn core_methods() -> Vec<ara_proto::MethodInfo> {
    let mut db = ara_schema::SchemaDb::new();
    let services: [Box<dyn Service>; 4] = [
        Box::new(core::Core),
        Box::new(profile::ProfileService),
        Box::new(diag::DiagService),
        Box::new(why::WhyService),
    ];
    let mut out = Vec::new();
    for service in services {
        for m in service.methods(&mut db) {
            out.push(ara_proto::MethodInfo {
                params: db
                    .get(&m.params)
                    .map(|t| db.type_json_schema(t))
                    .unwrap_or_default(),
                name: m.name,
                summary: m.summary,
                scope: m.scope,
                hints: m.hints,
                tool: m.tool,
                service: service.name().into(),
            });
        }
    }
    out
}
