//! Ara's retained native UI runtime, shared by editor, screen and world surfaces.
#![warn(missing_docs)]
pub mod accessibility;
pub mod binding;
pub mod collection;
pub mod controls;
pub mod cursor;
pub mod dialogs;
pub mod docking;
pub mod drag_drop;
mod emit;
mod focus;
pub mod input;
pub mod layers;
pub mod native_input;
pub mod paint;
mod pipeline;
pub mod popover;
pub mod reconcile;
pub mod runtime;
pub mod scroll;
pub mod select;
pub mod selection;
pub mod style;
pub mod theme;
pub mod virtualize;
mod widget_paint;
pub mod widgets;
mod world;
pub use ara_ui_document as source;
pub use reconcile::Reconciled;
pub use runtime::{Ui, Work};
/// UI runtime errors, including typed source diagnostics through their error source.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
