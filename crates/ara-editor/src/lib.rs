//! Ara's native editor: one authority, typed actions and a shared GPU UI/scene device.
#![warn(missing_docs)]

pub(crate) mod about;
mod actions;
pub mod activity;
pub(crate) mod animation_edit;
pub(crate) mod app;
pub(crate) mod asset_import;
pub(crate) mod asset_plan;
mod asset_service;
pub(crate) mod asset_tools;
pub(crate) mod assets;
pub(crate) mod authored;
mod authoring;
pub(crate) mod build;
mod capture;
pub(crate) mod catalog;
pub mod codes;
mod commands;
mod editor;
mod extensions;
mod fixes;
pub mod graph;
pub(crate) mod hub_launch;
pub(crate) mod ide;
mod interaction;
/// Editor-only Vulkan adapter over the shared engine device.
pub use ara_editor_gpu as gpu;
pub mod inspector;
pub(crate) mod lighting;
mod loading;
pub(crate) mod names;
mod native;
pub mod overlay;
pub mod paint;
pub(crate) mod painter;
mod persist;
pub(crate) mod play_controls;
pub(crate) mod play_profiling;
pub mod prefab;
mod probe_capture;
mod script_assets;
pub mod settings;
pub mod shell;
pub mod state;
pub mod studio;
pub(crate) mod system;
pub(crate) mod text;
mod transaction_label;
mod transport;
mod view_camera;
pub(crate) mod vfx_edit;
pub mod world;

pub use app::{LAUNCHED, Options, Settle, before_open_project, run};
pub use editor::{Editor, EditorError};
#[cfg(test)]
pub(crate) fn test_target() -> std::path::PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}
#[doc(hidden)]
pub use script_assets::worker::compile_scripts;

pub(crate) fn invalid(message: impl Into<String>) -> ara_diag::Diagnostic {
    ara_diag::Diagnostic::error(ara_diag::codes::INVALID_PARAMS, message)
}
/// Applies the Game view's Play settings to `play.start` params (tests only).
#[doc(hidden)]
pub fn testing_embed_play(editor: &Editor, params: &mut serde_json::Value) {
    editor.embed_play(params);
}
