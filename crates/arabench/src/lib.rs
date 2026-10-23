//! arabench: Ara's agent-ergonomics benchmark (R03 §4.13, M1 "arabench v0").
//!
//! A [`task::Task`] is a fixture project, a prompt written like a user request,
//! budgets, a deterministic verifier and a reference solution. The
//! [`runner`] gives each attempt a fresh `ara init` project in a temp dir, drives a
//! headless agent CLI ([`agent`]: Claude Code or Codex) against its MCP server, parses
//! the event stream ([`transcript`]), reopens the finished project ([`world`]) and
//! verifies it. Results are JSON ([`record`]) plus markdown ([`report`]).
//!
//! `arabench selftest` runs every reference solution instead of an agent, proving each
//! verifier rejects the untouched fixture and accepts a correct solution.

pub mod agent;
pub mod fixture;
pub mod record;
pub mod report;
pub mod runner;
pub mod solve;
pub mod suite;
pub mod task;
pub mod transcript;
pub mod world;
