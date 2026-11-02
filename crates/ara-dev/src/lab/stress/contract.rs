//! Render-v3 wire contract. Also compiled by the fidelity runner example so the
//! lab CLI does not acquire a renderer/GPU dependency merely to read a report.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: u32 = 1;

/// Authored deterministic fixture recipe; dimensions and light ranges are metres.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub gates: Vec<u8>,
    pub recipe: String,
    pub frames: u32,
    pub event_frame: u32,
    pub bodies: u32,
    pub triangles: u32,
    pub lights: u32,
    pub range_m: f32,
    pub target_lpp: Option<f64>,
    pub sun_deg_s: f32,
    pub elevation_deg: f32,
    pub contact_m: f32,
    pub reference_frames: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suite {
    pub version: u32,
    pub fps: u32,
    pub cases: Vec<Case>,
}

/// Identical frame script is consumed by both runs. No wall clock enters motion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameScript {
    pub frame: u32,
    pub seconds: f64,
    pub event_age: Option<u32>,
    pub camera_cut: bool,
    pub cold_residency: bool,
}

impl Case {
    pub fn script(&self, fps: u32) -> Vec<FrameScript> {
        (0..self.frames)
            .map(|frame| FrameScript {
                frame,
                seconds: f64::from(frame) / f64::from(fps),
                event_age: frame.checked_sub(self.event_frame),
                camera_cut: matches!(self.recipe.as_str(), "camera_cut" | "cold_cut" | "combined")
                    && frame == self.event_frame,
                cold_residency: matches!(self.recipe.as_str(), "cold_cut" | "combined")
                    && frame == self.event_frame,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Uncapped,
    Capped,
}

/// Missing instrumentation is null (rendered as n/a), never an invented zero.
/// Units of work are explicitly named; debt must survive subsequent events.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageCounters {
    pub gpu_ms: Option<f64>,
    pub cap_ms: Option<f64>,
    pub demand: Option<u64>,
    pub debt: Option<u64>,
    pub work_unit: Option<String>,
    pub oldest_debt_frames: Option<u32>,
    pub edge_lag_page_texels: Option<f64>,
    pub near_edge_lag_page_texels: Option<f64>,
    pub deadline_misses: Option<u64>,
    pub resident_bytes: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Quality {
    pub region_flip: BTreeMap<String, f64>,
    pub region_flip_excess: Option<f64>,
    pub max_solution_error_white: Option<f64>,
    pub temporal_block_rms_white: Option<f64>,
    pub temporal_blocks: Vec<f64>,
    pub penumbra_relative_error: Option<f64>,
    pub contact_displacement_px: Option<f64>,
    pub missing_shadow_features: Option<u64>,
    pub leak_pixels: Option<u64>,
    pub pop_events: Option<u64>,
    pub highlight_retention: Option<f64>,
    pub shadow_retention: Option<f64>,
    pub omitted_energy_fraction: Option<f64>,
    pub secondary_views_scored: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameResult {
    pub frame: u32,
    pub stages: BTreeMap<String, StageCounters>,
    pub gpu_frame_ms: Option<f64>,
    pub cpu_critical_path_ms: Option<f64>,
    pub submission_stall_ms: Option<f64>,
    pub host_update_render_ms: Option<f64>,
    pub lpp: Option<f64>,
    pub receiver_coverage: Option<f64>,
    pub quality: Quality,
    pub capture: Option<String>,
    pub reference: Option<String>,
    pub evidence: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub version: u32,
    pub case: Case,
    pub run: RunKind,
    pub renderer: String,
    pub device: String,
    pub revision: String,
    pub size: [u32; 2],
    pub fps: u32,
    pub uncapped_supported: bool,
    pub caps_applied: bool,
    pub timing_valid: bool,
    pub reference_samples: u32,
    pub reference_bounces: u32,
    pub reference_converged: bool,
    pub reference_white: f64,
    pub notes: Vec<String>,
    pub frames: Vec<FrameResult>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Distribution {
    pub count: usize,
    pub average: f64,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
}

/// Nearest-rank percentiles; empty/nonfinite input never creates passing numbers.
pub fn distribution(values: impl IntoIterator<Item = f64>) -> Option<Distribution> {
    let mut values: Vec<_> = values.into_iter().collect();
    if values.is_empty() || values.iter().any(|v| !v.is_finite() || *v < 0.) {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let count = values.len();
    Some(Distribution {
        count,
        average: values.iter().sum::<f64>() / count as f64,
        p50: values[count.div_ceil(2) - 1],
        p99: values[(count * 99).div_ceil(100) - 1],
        max: values[count - 1],
    })
}
