#![warn(missing_docs)]
//! SDL3 native window/input and Vulkan presentation for Ara.
mod output;
mod present;
mod window;
pub use present::Present;
pub use window::{Event, Window};
