//! Canonical component stories shared by native previews, agents and deterministic UI tests.
#![warn(missing_docs)]
mod book;
mod catalogue;
mod locale;
mod model;
mod mount;
pub use book::{StoryBook, register};
pub use catalogue::catalogue;
pub use locale::localize;
pub use model::*;
pub use mount::localize_markup;
/// Story validation and native runtime errors.
pub type Result<T> = ara_ui::Result<T>;
