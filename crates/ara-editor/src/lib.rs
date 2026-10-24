//! Ara's native editor: one authority, typed actions and a shared GPU UI/scene device.
#![warn(missing_docs)]

mod actions;
pub mod activity;
mod app;
mod asset_tools;
mod authoring;
mod capture;
pub mod codes;
mod commands;
mod editor;
mod fixes;
pub mod graph;
/// Editor-only Vulkan adapter over the shared engine device.
pub use ara_editor_gpu as gpu;
pub mod inspector;
mod native;
pub mod settings;
pub mod shell;
pub mod state;
mod transport;
pub mod world;

pub use app::{Options, run};
pub use editor::{Editor, EditorError};

pub(crate) fn invalid(message: impl Into<String>) -> ara_diag::Diagnostic {
    ara_diag::Diagnostic::error(ara_diag::codes::INVALID_PARAMS, message)
}
/// Applies the Game view's Play settings to `play.start` params (tests only).
#[doc(hidden)]
pub fn testing_embed_play(editor: &Editor, params: &mut serde_json::Value) {
    editor.embed_play(params);
}
