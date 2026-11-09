//! `trace()`: one ray query for every non-primary visibility and lighting caller
//! (council-v3 architecture D1). Software two-level CWBVHs: a TLAS over instances and
//! one BLAS per asset, built at import or on first load and cached by mesh hash.
//! Hardware RT is an optional backend of the same interface, off by default (not
//! built; `shaders/trace.slangh` documents the seam). Owned by the A1 build track.
//!
//! Use:
//! 1. Build each mesh's [`TraceMesh`] ([`mesh_from_geometry`] through a
//!    [`BlasCache`], or from an `ara_bvh::Blas`), [`TraceScene::add_mesh`] it, and
//!    [`TraceScene::set_instance`] every instance under a stable caller id.
//! 2. [`TraceScene::commit`], then [`TraceGpu::new`] / [`TraceGpu::update`].
//! 3. In Slang, `#include "trace.slangh"`, load `traceScene(record)` with
//!    [`TraceGpu::record`] and call `traceClosest` / `traceAny`; declare
//!    [`TraceGpu::buffers`] in the pass's graph.
//! 4. Each frame: `set_transform` for movers, `commit` (refit or rebuild), `update`.
//!
//! Hits report the instance slot ([`InstanceKey::slot`]), the source triangle and its
//! submesh; [`TraceScene::primitive`] and [`TraceScene::tombstones_since`] map them to
//! stable [`PrimitiveKey`]s and report what removals and mesh swaps killed.

// Wave 1 ships the query and its data; wave 2 adds the production callers.
#![allow(dead_code)]

mod cache;
mod gpu;
mod scene;

#[cfg(test)]
mod tests;

#[allow(unused_imports)] // wave 2 callers use these.
pub(crate) use cache::{BlasCache, Source, mesh_from_geometry};
#[allow(unused_imports)]
pub(crate) use gpu::{SceneRecord, TraceGpu, Upload};
#[allow(unused_imports)]
pub(crate) use scene::{
    AlphaMaterial, BlasKey, Commit, Death, InstanceKey, NO_MATERIAL, PrimitiveKey, Tombstone,
    TraceInstance, TraceMesh, TraceScene,
};
