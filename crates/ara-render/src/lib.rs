#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Ara's render graph, PBR baseline and deterministic visual feedback API.
//! Scene, instanced content, look and capture records live in `ara-frame`.
//! Callers outside this crate import those from `ara_frame`. These two
//! re-exports remain for this crate's own `use crate::{scene, capture}` paths.
//! The render graph lives in `ara-render2d` and is re-exported here.
pub use ara_frame::capture;
pub use ara_render2d::graph;
pub mod instanced;
#[cfg(feature = "fidelity-lab")]
pub mod lab;
pub mod look;
pub mod renderer;
pub use ara_frame::scene;
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
