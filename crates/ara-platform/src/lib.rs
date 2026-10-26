#![warn(missing_docs)]
//! SDL3 native window/input and Vulkan presentation for Ara.
mod chrome;
mod output;
mod present;
mod window;
pub use chrome::WindowChrome;
pub use present::Present;
pub use window::{Event, Window};
