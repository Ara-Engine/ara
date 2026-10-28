//! Cached, four-Hz text from observed CPU/renderer counters; absent is not zero.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatsSample {
    /// Wall time of the owner's last completed CPU loop, not simulation dt.
    pub cpu_loop_ms: Option<f64>,
    /// Maximum one completed renderer pass; this is not a total GPU frame time.
    pub gpu_longest_pass_ms: Option<f64>,
    pub gpu_pass_samples: Option<usize>,
    pub render_instances: Option<usize>,
    /// Shared native geometry buffer payload, excluding acceleration structures.
    pub geometry_payload_bytes: Option<u64>,
    /// Compressed texture payload, not total device memory usage.
    pub compressed_texture_bytes: Option<u64>,
    pub extent: Option<[u32; 2]>,
}
impl StatsSample {
    /// Read existing completed measurements. Call through observe_stats' lazy sampler.
    pub fn read(
        renderer: &ara_render::Renderer,
        cpu_loop_ms: Option<f64>,
        instances: Option<usize>,
        extent: Option<[u32; 2]>,
    ) -> Self {
        let mut passes = 0;
        let mut longest = None;
        for timing in renderer.timings() {
            if valid_ms(timing.milliseconds) {
                passes += 1;
                longest = Some(longest.map_or(timing.milliseconds, |value: f64| {
                    value.max(timing.milliseconds)
                }));
            }
        }
        Self {
            cpu_loop_ms: cpu_loop_ms.filter(|value| valid_ms(*value)),
            gpu_longest_pass_ms: longest,
            gpu_pass_samples: (passes > 0).then_some(passes),
            render_instances: instances,
            geometry_payload_bytes: renderer.native_geometry_stats().map(|stats| stats.bytes),
            compressed_texture_bytes: renderer
                .native_texture_stats()
                .map(|stats| stats.resident_bytes),
            extent,
        }
    }
    fn checked(mut self) -> Self {
        self.cpu_loop_ms = self.cpu_loop_ms.filter(|value| valid_ms(*value));
        self.gpu_longest_pass_ms = self.gpu_longest_pass_ms.filter(|value| valid_ms(*value));
        self.extent = self.extent.filter(|extent| extent[0] > 0 && extent[1] > 0);
        self
    }
}
fn valid_ms(value: f64) -> bool {
    value.is_finite() && value >= 0.
}

#[derive(Default)]
pub(super) struct Stats {
    sample: Option<StatsSample>,
    rows: [String; 6],
    elapsed: Duration,
    active: bool,
    samples: u64,
}
impl Stats {
    pub fn observe(
        &mut self,
        elapsed: Duration,
        enabled: bool,
        sample: impl FnOnce() -> StatsSample,
    ) -> bool {
        if !enabled {
            self.active = false;
            self.elapsed = Duration::ZERO;
            return false;
        }
        self.elapsed = self.elapsed.saturating_add(elapsed);
        if self.active && self.elapsed < Duration::from_millis(250) {
            return false;
        }
        self.active = true;
        self.elapsed = Duration::ZERO;
        let sample = sample().checked();
        let rows = rows(sample);
        let changed = rows != self.rows;
        self.rows = rows;
        self.sample = Some(sample);
        self.samples += 1;
        changed
    }
    pub fn markup(
        &self,
        flags: Flags,
        counts: &Counts,
        current: bool,
        extent: Option<[f32; 2]>,
        out: &mut String,
    ) {
        let extent = extent.filter(|extent| extent.iter().all(|n| n.is_finite() && *n > 0.));
        if flags.stats {
            let [w, h] = extent.unwrap_or([280., 220.]);
            let top = if h < 140. { 8. } else { 46. };
            let inset = if w < 100. { 4. } else { 12. };
            let width = (w - inset * 2.).clamp(8., 236.);
            let height = (h - top - 8.).max(8.);
            // Anchor to the same measured Scene extent used by projection. A
            // right inset on an intrinsically sized host can resolve against a
            // zero-width containing block and place the HUD outside the Scene.
            let left = (w - inset - width).max(0.);
            out.push_str(&format!("<scroll id='scene-diagnostic-stats' style='position:absolute;left:{left}px;top:{top}px;width:{width}px;min-width:0px;height:auto;max-height:{height}px;flex-basis:auto;min-height:0px;padding:8px;gap:4px;background:var(--popover);border-color:var(--border);border-width:1px;border-radius:6px'><text id='scene-diagnostic-stats-title' class='caption' style='white-space:normal;flex-shrink:0' text='Last completed samples' />"));
            if self.sample.is_none() {
                out.push_str("<text id='scene-diagnostic-stats-pending' class='caption' text='Statistics unavailable until sampled' />");
            } else {
                for (index, row) in self.rows.iter().enumerate() {
                    out.push_str(&format!("<text id='scene-diagnostic-stat-{index}' class='caption' style='white-space:normal;flex-shrink:0' text='{}' />",crate::shell::escape(row)));
                }
            }
            out.push_str("</scroll>");
        }
        if flags.bounds || flags.colliders {
            let mut notes = Vec::new();
            if flags.bounds && !current {
                notes.push("Mesh bounds await current native content".into());
            }
            if counts.pending_models > 0 {
                notes.push(format!(
                    "{} loading model bounds unavailable",
                    counts.pending_models
                ));
            }
            if counts.unavailable_bounds > 0 {
                notes.push(format!(
                    "{} mesh bounds lack native provenance",
                    counts.unavailable_bounds
                ));
            }
            if counts.unsupported_colliders > 0 {
                notes.push(format!(
                    "{} cooked/model collider shapes unavailable",
                    counts.unsupported_colliders
                ));
            }
            if counts.invalid_colliders + counts.invalid_bounds > 0 {
                notes.push(format!(
                    "{} malformed shapes skipped",
                    counts.invalid_colliders + counts.invalid_bounds
                ));
            }
            if !notes.is_empty() {
                let width = extent.map_or(320., |[width, _]| (width - 24.).clamp(16., 360.));
                out.push_str(&format!("<panel id='scene-diagnostic-notice' style='position:absolute;left:12px;bottom:12px;max-width:{width}px;padding:6px;background:var(--popover);border-radius:4px'><text id='scene-diagnostic-notice-text' class='caption' style='white-space:normal' text='{}' /></panel>",crate::shell::escape(&notes.join(" · "))));
            }
        }
    }
    pub fn report(&self) -> Value {
        json!({"samples":self.samples,"sample":self.sample,"cadenceMs":250,
        "scope":"CPU owner loop wall time and last completed renderer pass/native payload counters; no inferred FPS, GPU frame total, draw calls or total VRAM"})
    }
}
fn rows(sample: StatsSample) -> [String; 6] {
    let ms = |value: Option<f64>| {
        value.map_or_else(|| "unavailable".into(), |value| format!("{value:.2} ms"))
    };
    let bytes = |value: Option<u64>| {
        value.map_or_else(
            || "unavailable".into(),
            |value| format!("{:.2} MiB", value as f64 / (1024. * 1024.)),
        )
    };
    [
        format!("CPU loop wall: {}", ms(sample.cpu_loop_ms)),
        format!(
            "GPU longest pass: {} ({} samples)",
            ms(sample.gpu_longest_pass_ms),
            sample
                .gpu_pass_samples
                .map_or_else(|| "unknown".into(), |count| count.to_string())
        ),
        format!(
            "Native instances: {}",
            sample
                .render_instances
                .map_or_else(|| "unknown".into(), |count| count.to_string())
        ),
        format!("Geometry payload: {}", bytes(sample.geometry_payload_bytes)),
        format!(
            "Compressed textures: {}",
            bytes(sample.compressed_texture_bytes)
        ),
        format!(
            "Scene output: {}",
            sample
                .extent
                .map_or_else(|| "unknown".into(), |[w, h]| format!("{w} × {h} px"))
        ),
    ]
}
