#![warn(missing_docs)]
//! SDL3 native window/input and Vulkan presentation for Ara.
mod cursor;
mod output;
mod present;
mod window;
pub use cursor::SystemCursor;
pub use present::Present;
pub use window::{Event, Window};
