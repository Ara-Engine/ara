use ara_gpu::commands::PassTiming;
use serde::Serialize;

/// Live statistics for one emitter, directly read from GPU counters.
#[derive(Debug, Serialize)]
pub struct EmitterStats {
    /// Stable authoring name.
    pub name: String,
    /// Live particles after update/compaction.
    pub alive: u32,
    /// Available slots under the quality cap.
    pub dead: u32,
    /// Visible instances after the overdraw cap.
    pub rendered: u32,
    /// Cosmetic events retained for next-tick routing.
    pub events: u32,
    /// Cumulative demand dropped at capacity.
    pub dropped_spawns: u32,
    /// Cumulative events/children omitted by quality caps.
    pub dropped_events: u32,
}
/// Agent-readable report; every GPU stage time is an actual timestamp delta.
#[derive(Debug, Serialize)]
pub struct Stats {
    /// Completed deterministic ticks.
    pub tick: u32,
    /// Total live particle count.
    pub particles_alive: u32,
    /// GPU counters by emitter.
    pub emitters: Vec<EmitterStats>,
    /// Measured GPU pass times for the last submitted frame.
    pub gpu_stages: Vec<PassTiming>,
    /// Conservative requested screen coverage, in full-screen equivalents.
    pub overdraw_estimate: f32,
    /// Indirect geometry batches after CPU bounds culling.
    pub batches: u32,
    /// Exported emitter lights after caps.
    pub lights: u32,
    /// Actual physical device and driver metadata.
    pub device: serde_json::Value,
    /// Deterministic effect content identity.
    pub effect_hash: String,
    /// Same-queue or dedicated compute execution.
    pub async_compute: bool,
    /// Human/agent explanations for quality reductions.
    pub diagnostics: Vec<String>,
}
