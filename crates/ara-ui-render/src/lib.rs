//! Ordered UI batching, glyph atlases and an engine-neutral GPU adapter boundary.
#![warn(missing_docs)]
mod cpu;
mod msdf;
mod reference;
pub use reference::CpuGpu;
pub mod atlas;
pub mod gpu;
/// Deterministic offline reference backend, shared by goldens and designer captures.
pub struct CpuRenderer;
