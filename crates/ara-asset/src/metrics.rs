//! Optional offline profiling; never participates in content identity.
use serde::Serialize;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

/// Timed import phases. Builtin batch cooks time each complete parallel phase once.
#[derive(Debug, Clone, Copy)]
#[repr(usize)]
pub enum ImportStage {
    /// Source image decoding and pixel preparation.
    Decode,
    /// Filtered mip construction.
    Mips,
    /// Pixel packing and native block compression.
    Compression,
    /// Mesh optimization, quantization and meshlets.
    Mesh,
    /// Cooked payload validation.
    Validation,
    /// Source model parsing, excluding image preparation and geometry processing.
    Parse,
    /// Summed classic LOD work; can overlap across primitives inside Mesh.
    Lods,
    /// Companion opacity-map decode and combination.
    Opacity,
    /// CPU quantization and block packing for the optional GPU encoder.
    GpuPacking,
    /// LOD topology, seam/card classification and attribute preparation.
    LodPreparation,
    /// Geometric and attribute-aware simplification calls, excluding area checks.
    LodSimplification,
    /// Per-card area checks on candidate LODs.
    LodArea,
    /// Virtual-geometry construction and encoding, included in Mesh.
    VirtualGeometry,
}
/// Thread-safe, per-job timers. No global counters and no effect on output bytes.
#[derive(Debug, Default)]
pub struct ImportMetrics {
    nanos: [AtomicU64; 13],
    texture_tasks: [AtomicU64; 2],
}
/// Summed elapsed stage intervals in milliseconds. Overlapping plugin spans may exceed wall time.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageTimes {
    /// Texture tasks actually encoded; unchanged memo hits do not count.
    pub texture_cooks: u64,
    /// Texture tasks reused from the supplied content-addressed memo.
    pub texture_cache_hits: u64,
    /// Image decode and preparation work.
    pub decode_ms: f64,
    /// Mip filtering work.
    pub mips_ms: f64,
    /// Block compression and packing work.
    pub compression_ms: f64,
    /// Geometry processing work.
    pub mesh_ms: f64,
    /// Payload validation work.
    pub validation_ms: f64,
    /// Model source parsing.
    pub parse_ms: f64,
    /// Classic LOD work across parallel primitives, included in mesh time.
    pub lods_ms: f64,
    /// Companion opacity preparation.
    pub opacity_ms: f64,
    /// CPU packing for GPU compression, included in compression time.
    pub gpu_packing_ms: f64,
    /// Topology/attribute preparation, nested in classic LOD work.
    pub lod_preparation_ms: f64,
    /// Simplifier calls, nested in classic LOD work.
    pub lod_simplification_ms: f64,
    /// Card-area checks, nested in classic LOD work.
    pub lod_area_ms: f64,
    /// Virtual-geometry construction/encoding, nested in Mesh.
    pub virtual_geometry_ms: f64,
}
/// Records one scoped interval even if the stage fails.
pub struct StageTimer<'a> {
    metrics: &'a ImportMetrics,
    stage: ImportStage,
    start: Instant,
}
impl ImportMetrics {
    /// Records completed texture work separately from reused results.
    pub fn texture_task(&self, cache_hit: bool) {
        self.texture_tasks[usize::from(cache_hit)].fetch_add(1, Ordering::Relaxed);
    }
    /// Starts an elapsed stage interval. Time the batch around its parallel tasks.
    pub fn start(&self, stage: ImportStage) -> StageTimer<'_> {
        StageTimer {
            metrics: self,
            stage,
            start: Instant::now(),
        }
    }
    /// Takes a consistent-enough observation after the job has joined its tasks.
    pub fn snapshot(&self) -> StageTimes {
        let n = self
            .nanos
            .each_ref()
            .map(|v| v.load(Ordering::Relaxed) as f64 / 1e6);
        StageTimes {
            texture_cooks: self.texture_tasks[0].load(Ordering::Relaxed),
            texture_cache_hits: self.texture_tasks[1].load(Ordering::Relaxed),
            decode_ms: n[0],
            mips_ms: n[1],
            compression_ms: n[2],
            mesh_ms: n[3],
            validation_ms: n[4],
            parse_ms: n[5],
            lods_ms: n[6],
            opacity_ms: n[7],
            gpu_packing_ms: n[8],
            lod_preparation_ms: n[9],
            lod_simplification_ms: n[10],
            lod_area_ms: n[11],
            virtual_geometry_ms: n[12],
        }
    }
}
impl Drop for StageTimer<'_> {
    fn drop(&mut self) {
        self.metrics.nanos[self.stage as usize].fetch_add(
            self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}
