#![warn(missing_docs)]
//! SDL3 native window/input and native presentation (Vulkan, or Metal on macOS) for Ara.
mod chrome;
mod cursor;
#[cfg(not(target_os = "macos"))]
mod output;
mod present;
pub mod text;
mod window;
pub use chrome::WindowChrome;
pub use cursor::SystemCursor;
pub use present::Present;
pub use window::{Event, GameWindowMode, Window};
