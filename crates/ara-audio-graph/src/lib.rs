//! Procedural audio graphs for Ara: MetaSounds-class sound design with a smaller,
//! text-first surface.
//!
//! - [`ast`]: the single program (`AudioGraph`); names for authors, IDs for identity.
//! - [`dsl`]: the compact `.audiograph` text view (no IDs), `parse`/`reconcile`/`print`.
//! - [`projection`]: the `ara-graph` view (domain `audio`) and [`AudioAdapter`].
//! - [`commands`]: typed, undoable edits by name (`audiograph.*`).
//! - [`check`] → [`compile`] → [`Program`]: a flat instruction list over a buffer arena.
//! - [`runtime`]: [`GraphVoice`] (an `ara_audio::SignalSource`) and [`GraphHandle`]
//!   (sample-accurate parameters/triggers, per-node activity for captures).
//! - [`library`]: every operation's documented signature and cost.

pub mod ast;
pub mod check;
pub mod commands;
pub mod compile;
pub mod dsl;
pub mod library;
pub mod projection;
pub mod runtime;

pub use ast::{AudioGraph, Type};
pub use check::{Checked, check};
pub use compile::{ClipSource, Program, compile};
pub use library::Op;
pub use projection::AudioAdapter;
pub use runtime::{GraphHandle, GraphVoice};

use ara_diag::Diagnostic;
use std::sync::Arc;

/// Diagnostic codes (shared with ara-audio's audio range).
pub mod codes {
    /// Audio graph parse, check or compile failure (ARA3406).
    pub use ara_audio::diagnose::codes::GRAPH;
}

/// Parses, checks and compiles `.audiograph` text in one step (loader thread).
pub fn load(
    source: &str,
    asset_id: &str,
    sample_rate: u32,
    clips: &mut dyn ClipSource,
) -> Result<Arc<Program>, Vec<Diagnostic>> {
    let graph = dsl::parse(source, asset_id)?;
    let checked = check(&graph)?;
    compile(&checked, sample_rate, clips)
}
