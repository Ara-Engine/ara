use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;
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
    /// Content key of the exact offline shader cohort used for these measurements.
    pub shader_key: String,
    /// Same-queue or dedicated compute execution.
    pub async_compute: bool,
    /// Human/agent explanations for quality reductions.
    pub diagnostics: Vec<String>,
}

impl crate::GpuSystem {
    /// Read GPU counters and quality diagnostics after the most recent completed frame.
    pub fn stats(&self, gpu: &GpuDevice) -> Result<Stats> {
        ensure!(self.tick > 0, "step before querying GPU stats");
        let bytes = gpu.read(self.read_control)?;
        let words = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect::<Vec<_>>();
        let emitters = self
            .effect
            .emitters
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let w = &words[i * 16..];
                EmitterStats {
                    name: e.name.clone(),
                    alive: w[0],
                    dead: w[1],
                    rendered: w[14],
                    events: w[2],
                    dropped_spawns: w[12],
                    dropped_events: w[13],
                }
            })
            .collect::<Vec<_>>();
        let overdraw_estimate = emitters
            .iter()
            .enumerate()
            .map(|(i, _)| f32::from_bits(words[i * 16 + 15]))
            .sum();
        let mut diagnostics = self
            .effect
            .diagnostics()
            .iter()
            .map(|d| format!("{d:?}"))
            .collect::<Vec<_>>();
        for e in &emitters {
            if e.dropped_spawns > 0 {
                diagnostics.push(format!(
                    "{}: {} spawns dropped at capacity",
                    e.name, e.dropped_spawns
                ));
            }
            if e.dropped_events > 0 {
                diagnostics.push(format!(
                    "{}: {} events/children dropped by quality cap",
                    e.name, e.dropped_events
                ));
            }
            if e.rendered < e.alive {
                diagnostics.push(format!(
                    "{}: render instances limited by culling/overdraw",
                    e.name
                ));
            }
        }
        Ok(Stats {
            tick: self.tick,
            particles_alive: emitters.iter().map(|e| e.alive).sum(),
            emitters,
            gpu_stages: self.timings.to_vec(),
            overdraw_estimate,
            batches: self.visible,
            lights: self
                .layout
                .configs
                .iter()
                .filter(|c| c.light_color[3] > 0.)
                .count() as u32,
            device: ara_json::to_value(gpu.info())?,
            effect_hash: self.hash.clone(),
            shader_key: self.metadata.content_key.clone(),
            async_compute: self.async_compute,
            diagnostics,
        })
    }
}
