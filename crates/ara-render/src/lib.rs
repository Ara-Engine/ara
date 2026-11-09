#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Ara's render graph, PBR baseline and deterministic visual feedback API.
pub mod capture;
pub mod graph;
pub mod instanced;
#[cfg(feature = "fidelity-lab")]
pub mod lab;
pub mod look;
pub mod renderer;
pub mod scene;
mod tsr;
pub use look::reflect::{ReflectionCounters, SoftwareReflections};
pub use look::transport::solve::{
    PatchTransport, TransportMedium, TransportOptions, TransportReport,
};
pub use look::vsm::sun::{Counters as SunPageStage, SunPageCounters, SunPages};
pub use renderer::{
    HdrExposure, NativeContent, NativeSeed, Outlines, ReferenceTrace, ReflectionView, RenderDebug,
    Renderer, ViewId, ViewOutput, ViewRequest, temporal_phases,
};
pub use tsr::Upscaler;
