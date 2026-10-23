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
}
/// Thread-safe, per-job timers. No global counters and no effect on output bytes.
#[derive(Debug, Default)]
pub struct ImportMetrics {
    nanos: [AtomicU64; 5],
}
/// Summed elapsed stage intervals in milliseconds. Overlapping plugin spans may exceed wall time.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageTimes {
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
}
/// Records one scoped interval even if the stage fails.
pub struct StageTimer<'a> {
    metrics: &'a ImportMetrics,
    stage: ImportStage,
    start: Instant,
}
impl ImportMetrics {
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
            decode_ms: n[0],
            mips_ms: n[1],
            compression_ms: n[2],
            mesh_ms: n[3],
            validation_ms: n[4],
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
