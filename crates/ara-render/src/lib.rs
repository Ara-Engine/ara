#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Ara's render graph, PBR baseline and deterministic visual feedback API.
pub mod capture;
pub mod graph;
pub mod instanced;
pub mod look;
pub mod renderer;
pub mod scene;
pub use renderer::{
    HdrExposure, NativeContent, NativeSeed, Renderer, ViewId, ViewOutput, ViewRequest,
    temporal_phases,
};
