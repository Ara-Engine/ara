#![warn(missing_docs)]
//! SDL3 native window/input and native presentation (Vulkan, or Metal on macOS) for Ara.
mod chrome;
mod cursor;
mod live_repaint;
#[cfg(not(target_os = "macos"))]
mod output;
mod present;
pub mod text;
mod window;
pub use chrome::WindowChrome;
pub use cursor::SystemCursor;
pub use live_repaint::with_live_repaint;
pub use present::{Present, out_of_date};
pub use window::{Event, GameWindowMode, MonitorMode, Window};
