//! Shared SDL3 bindings backed by Ara's vendored static build.
//!
//! Both platform and audio depend on this crate, so Cargo links exactly one SDL.
pub use sdl3_sys::*;
