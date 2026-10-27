//! Opt-in frame windows and a file control queue; ordinary exported games retain their CLI behavior.
use anyhow::{Context, Result, bail, ensure};
use ara_gpu::vulkan::PassTiming;
use ara_runtime_render::Presenter;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod arrival;
mod boundary;
mod cpu;
mod gpu;
mod hitches;
mod lab;
mod report;
mod span;

pub(super) struct CpuFrame {
    pub phases: [f64; 4],
    pub total_ms: f64,
    pub wait_ms: Option<f64>,
    pub first_presented: bool,
}

#[derive(clap::Args)]
pub(super) struct PerfOptions {
    /// Publish detailed frame-window metrics to this JSON file (development only).
    #[arg(long)]
    perf_output: Option<PathBuf>,
    #[arg(long, default_value_t = 240, value_parser = clap::value_parser!(u32).range(1..))]
    perf_frames: u32,
    #[arg(long, default_value_t = 30)]
    perf_warmup: u32,
    /// Wait for settled raster content, texture residency and converged GI before warmup.
    #[arg(long, requires = "perf_output")]
    perf_wait_ready: bool,
    /// Fail the measurement if readiness has not arrived within this many milliseconds.
    #[arg(long, default_value_t = 120_000, value_parser = clap::value_parser!(u64).range(1..))]
    perf_ready_timeout_ms: u64,
    /// JSON request queue: quality, reload_shaders, measure and stop.
    #[arg(long, requires = "perf_output")]
    perf_control: Option<PathBuf>,
    #[arg(long, requires = "perf_output")]
    keep_alive: bool,
    /// Lab-owned player: retain GPU resources but render only requested windows.
    #[arg(long, requires_all = ["keep_alive", "perf_control"])]
    lab_idle: bool,
}

#[derive(Default, Deserialize)]
struct Control {
    request_id: Option<String>,
    frames: Option<u32>,
    camera: Option<lab::Camera>,
    capture: Option<lab::Capture>,
    #[cfg(feature = "fidelity-lab")]
    params: Option<Value>,
    #[cfg(feature = "fidelity-lab")]
    temporal: Option<lab::TemporalRequest>,
    quality: Option<String>,
    #[serde(default)]
    reload_shaders: bool,
    #[serde(default)]
    measure: bool,
    #[serde(default)]
    stop: bool,
}

impl PerfOptions {
    pub(super) fn requested(&self) -> bool {
        self.perf_output.is_some()
    }
}

pub(super) struct Window {
    options: PerfOptions,
    offscreen: bool,
    no_present: bool,
    windowed: bool,
    frames: BTreeMap<String, Vec<f64>>,
    passes: BTreeMap<String, Vec<f64>>,
    gpu: gpu::Samples,
    cpu: cpu::Samples,
    span: span::Samples,
    run_cpu: cpu::Samples,
    hitches: hitches::Samples,
    launched: Instant,
    last_report: Option<Value>,
    arrival: arrival::Samples,
    boundary: boundary::Boundary,
    began: Instant,
    from_launch: bool,
    ready_status: Option<ara_runtime_render::PresentationReadiness>,
    seen: u32,
    measured: u32,
    collecting: bool,
    next_control: Instant,
    waiting_for_shader_epoch: Option<u64>,
    last_epoch: Option<u64>,
    live_status: Vec<u8>,
    lab: lab::State,
}

fn quantiles(samples: &[f64]) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = |fraction: f64| {
        sorted[((sorted.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
    };
    json!({"p50":rank(0.5),"p95":rank(0.95),"p99":rank(0.99),"max":sorted[sorted.len()-1],"mean":sorted.iter().sum::<f64>()/sorted.len() as f64,"samples":sorted.len()})
}

impl Window {
    pub fn offscreen(&self) -> bool {
        self.offscreen
    }
    pub fn enabled(&self) -> bool {
        self.options.perf_output.is_some()
    }
    /// Label the no-WSI discriminator separately from presented measurements.
    pub fn configure_presentation(&mut self, no_present: bool, windowed: bool) {
        self.no_present = no_present;
        self.windowed = windowed;
        self.offscreen |= no_present;
    }
    pub fn new(options: PerfOptions, offscreen: bool, began: Instant) -> Self {
        let collecting = options.perf_output.is_some() && !options.lab_idle;
        let boundary = boundary::Boundary::new(
            options.perf_warmup,
            Duration::from_millis(options.perf_ready_timeout_ms),
        );
        Self {
            offscreen: offscreen || collecting,
            no_present: false,
            windowed: false,
            options,
            frames: BTreeMap::new(),
            passes: BTreeMap::new(),
            gpu: gpu::Samples::default(),
            cpu: cpu::Samples::default(),
            span: span::Samples::default(),
            run_cpu: cpu::Samples::default(),
            hitches: hitches::Samples::default(),
            launched: began,
            last_report: None,
            arrival: arrival::Samples::default(),
            boundary,
            began,
            from_launch: true,
            ready_status: None,
            seen: 0,
            measured: 0,
            collecting,
            next_control: Instant::now(),
            waiting_for_shader_epoch: None,
            last_epoch: None,
            live_status: Vec::new(),
            lab: lab::State::default(),
        }
    }

    fn clear_samples(&mut self) {
        self.frames.clear();
        self.passes.clear();
        self.gpu = gpu::Samples::default();
        self.cpu = cpu::Samples::default();
        self.span = span::Samples::default();
        self.measured = 0;
    }

    fn reset(&mut self) {
        self.clear_samples();
        self.arrival = arrival::Samples::default();
        self.boundary = boundary::Boundary::new(
            self.options.perf_warmup,
            Duration::from_millis(self.options.perf_ready_timeout_ms),
        );
        self.began = Instant::now();
        self.from_launch = false;
        self.ready_status = None;
        self.seen = 0;
        self.collecting = true;
    }

    pub fn control(&mut self, presenter: &mut Presenter) -> Result<bool> {
        if self.options.perf_output.is_none() || Instant::now() < self.next_control {
            return Ok(false);
        }
        self.next_control = Instant::now()
            + Duration::from_millis(if self.options.lab_idle && !self.collecting {
                10
            } else {
                100
            });
        let shaders = presenter.shader_status();
        if !shaders.pending && shaders.last_error.is_none() {
            let changed = self
                .last_epoch
                .replace(shaders.epoch)
                .is_some_and(|old| old != shaders.epoch);
            // The first observed epoch is initial state. An explicit reload
            // already started its window; retain its waiting CPU history/clock.
            if changed && self.waiting_for_shader_epoch.is_none() && !self.options.lab_idle {
                self.reset();
            }
        }
        if let Some(output) = &self.options.perf_output {
            if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let status = serde_json::to_vec_pretty(
                &json!({"quality":presenter.quality().map(|q| q.name()),"collecting":self.collecting,"shaders":shaders,"readiness":presenter.readiness()}),
            )?;
            if status != self.live_status {
                let path = output.with_file_name("status.json");
                let temporary = path.with_extension("json.tmp");
                std::fs::write(&temporary, &status)?;
                std::fs::rename(temporary, path)?;
                self.live_status = status;
            }
        }
        let Some(directory) = &self.options.perf_control else {
            return Ok(false);
        };
        std::fs::create_dir_all(directory)?;
        let mut requests: Vec<_> = std::fs::read_dir(directory)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        requests.sort();
        for request in requests {
            let command: Control = serde_json::from_slice(&std::fs::read(&request)?)?;
            self.lab.reply = command
                .request_id
                .as_ref()
                .map(|_| request.with_extension("reply"));
            if command.stop {
                std::fs::remove_file(request)?;
                return Ok(true);
            }
            if let Some(quality) = &command.quality {
                presenter.set_quality(quality.parse()?);
            }
            if let Some(frames) = command.frames {
                ensure!((1..=100000).contains(&frames), "invalid measurement frames");
                self.options.perf_frames = frames;
            }
            if let Some(capture) = command.capture {
                self.lab.capture = Some(capture);
            }
            #[cfg(feature = "fidelity-lab")]
            if let Some(request) = command.temporal {
                self.lab.temporal = Some(request);
            }
            #[cfg(not(feature = "fidelity-lab"))]
            let changing_params = false;
            #[cfg(feature = "fidelity-lab")]
            let changing_params = command.params.is_some();
            #[cfg(feature = "fidelity-lab")]
            let live_params = changing_params
                && self.options.lab_idle
                && !self.collecting
                && !command.measure
                && !command.reload_shaders
                && command.quality.is_none()
                && self.lab.capture.is_none()
                && self.lab.temporal.is_none();
            #[cfg(feature = "fidelity-lab")]
            let repaint = command.camera.is_some()
                || command
                    .params
                    .as_ref()
                    .is_some_and(|v| v.as_object().is_none_or(|o| !o.is_empty()));
            #[cfg(feature = "fidelity-lab")]
            if let Some(params) = command.params {
                let applied = self.lab.set_params(presenter, params);
                match applied {
                    Ok(report) => self.lab.param_report = Some(report),
                    Err(error) => {
                        self.lab.respond(&json!({"error":format!("{error:#}")}))?;
                        std::fs::rename(&request, request.with_extension("done"))?;
                        continue;
                    }
                }
                self.options.perf_frames = 1;
            }
            if let Some(camera) = command.camera {
                self.lab.camera = Some(camera.view()?);
            }
            #[cfg(feature = "fidelity-lab")]
            if live_params {
                self.lab.repaint = repaint;
                if !repaint {
                    let params = self.lab.param_report.take();
                    self.lab.respond(&json!({"applied":true,"params":params}))?;
                }
                std::fs::rename(&request, request.with_extension("done"))?;
                continue;
            }
            if command.reload_shaders {
                self.waiting_for_shader_epoch = Some(presenter.shader_status().epoch);
                presenter.reload_shaders();
            }
            if command.measure
                || changing_params
                || command.reload_shaders
                || (!self.options.lab_idle && command.quality.is_some())
            {
                self.reset();
            }
            if self.idle()
                || (!self.options.lab_idle && !self.collecting && self.lab.capture.is_none())
            {
                self.lab.respond(&json!({"applied":true}))?;
            }
            std::fs::rename(&request, request.with_extension("done"))?;
        }
        Ok(false)
    }

    pub fn frame(
        &mut self,
        presenter: &Presenter,
        phases: &BTreeMap<&str, u128>,
        camera: Value,
        frame: CpuFrame,
    ) -> Result<bool> {
        #[cfg(feature = "fidelity-lab")]
        if std::mem::take(&mut self.lab.repaint) {
            let params = self.lab.param_report.take();
            self.lab.respond(&json!({"applied":true,"repainted":true,"params":params,"shaders":presenter.shader_status()}))?;
        }
        if self.enabled() {
            self.run_cpu
                .record(frame.total_ms, frame.wait_ms, frame.first_presented);
            self.hitches.record(
                self.run_cpu.count(),
                self.launched.elapsed(),
                self.collecting,
                presenter,
                &frame,
            );
        }
        if !self.collecting {
            return Ok(false);
        }
        let mut shaders_ready = true;
        if self.waiting_for_shader_epoch.is_some() {
            let status = presenter.shader_status();
            if let Some(error) = status.last_error {
                eprintln!("Shader reload failed; previous result retained: {error}");
                self.collecting = false;
                self.waiting_for_shader_epoch = None;
                self.lab.respond(
                    &json!({"error":format!("shader reload failed; last good retained: {error}")}),
                )?;
                return Ok(false);
            }
            // An unchanged cohort completes without advancing the epoch.
            if status.pending {
                shaders_ready = false;
            } else {
                self.waiting_for_shader_epoch = None;
            }
        }
        if !shaders_ready && !self.options.perf_wait_ready {
            return Ok(false);
        }
        self.seen += 1;
        if self.options.perf_wait_ready {
            let status = presenter.readiness();
            if self.boundary.confirmed() && (status.failed || !status.raster || !status.gi_enabled)
            {
                self.publish(presenter, phases, camera, report::Outcome::Failed)?;
                bail!(
                    "perf presentation lost its eligible raster/GI path: {:?}; {:?}",
                    status,
                    presenter.presentation_error()
                );
            }
            let ready = shaders_ready && status.raster && status.gi_enabled && status.is_ready();
            if ready && !self.boundary.confirmed() {
                self.ready_status = Some(status);
            }
            match self
                .boundary
                .observe(self.seen, self.began.elapsed(), ready)
            {
                boundary::Step::Sample => {}
                boundary::Step::Waiting | boundary::Step::Warmup => {
                    if self.measured != 0 {
                        self.clear_samples();
                    }
                    self.arrival.record(presenter, &frame);
                    return Ok(false);
                }
                boundary::Step::TimedOut => {
                    self.arrival.record(presenter, &frame);
                    self.publish(presenter, phases, camera, report::Outcome::TimedOut)?;
                    bail!(
                        "perf readiness timed out after {} ms: {:?}; presentation error: {:?}",
                        self.options.perf_ready_timeout_ms,
                        status,
                        presenter.presentation_error()
                    );
                }
            }
        } else if self.seen <= self.options.perf_warmup {
            return Ok(false);
        }
        let total = frame.total_ms;
        self.cpu.record(total, frame.wait_ms, frame.first_presented);
        for (name, value) in [
            "simulation_and_extraction",
            "presentation_prepare",
            "presentation_including_display",
            "audio",
        ]
        .into_iter()
        .zip(frame.phases)
        {
            self.frames.entry(name.into()).or_default().push(value);
        }
        self.frames
            .entry("frame_ms".into())
            .or_default()
            .push(total);
        self.frames
            .entry("gpu_ms".into())
            .or_default()
            .push(presenter.stats().gpu_ms);
        self.frames
            .entry("fps".into())
            .or_default()
            .push(1000. / total.max(f64::MIN_POSITIVE));
        let mut per_frame: BTreeMap<&str, f64> = BTreeMap::new();
        for PassTiming {
            pass, milliseconds, ..
        } in presenter.pass_timings()
        {
            *per_frame.entry(pass).or_default() += milliseconds;
        }
        self.gpu.record(presenter.stats(), per_frame.values().sum());
        self.span.record(
            presenter.stats().gpu_span_ms,
            presenter.stats().gpu_submission_gap_ms,
            per_frame.values().sum(),
        );
        for (pass, milliseconds) in per_frame {
            self.passes
                .entry(pass.to_owned())
                .or_default()
                .push(milliseconds);
        }
        self.measured += 1;
        if self.measured < self.options.perf_frames {
            return Ok(false);
        }
        self.publish(presenter, phases, camera, report::Outcome::Ready)?;
        if let Some(report) = &self.last_report {
            #[cfg(feature = "fidelity-lab")]
            let params = self.lab.param_report.take();
            #[cfg(feature = "fidelity-lab")]
            self.lab
                .respond(&json!({"player":report,"params":params}))?;
            #[cfg(not(feature = "fidelity-lab"))]
            self.lab.respond(&json!({"player":report}))?;
        }
        self.collecting = false;
        Ok(!self.options.keep_alive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_measurement_never_opens_a_visible_window() -> Result<()> {
        use clap::Parser;
        let normal = crate::Args::try_parse_from(["ara-player"])?;
        assert!(
            !Window::new(
                normal.standalone.perf,
                normal.standalone.offscreen,
                Instant::now()
            )
            .offscreen()
        );
        let perf = crate::Args::try_parse_from(["ara-player", "--perf-output", "metrics.json"])?;
        assert!(
            Window::new(
                perf.standalone.perf,
                perf.standalone.offscreen,
                Instant::now()
            )
            .offscreen()
        );
        let hidden = crate::Args::try_parse_from(["ara-player", "--offscreen"])?;
        assert!(
            Window::new(
                hidden.standalone.perf,
                hidden.standalone.offscreen,
                Instant::now()
            )
            .offscreen()
        );
        assert!(crate::Args::try_parse_from(["ara-player", "--offscreen", "--headless"]).is_err());
        Ok(())
    }
    #[test]
    fn reports_real_quantiles_and_keeps_no_samples_missing() {
        assert!(quantiles(&[]).is_null());
        let stats = quantiles(&[4., 1., 2., 3.]);
        assert_eq!(stats["p50"], 2.);
        assert_eq!(stats["p95"], 4.);
        assert_eq!(stats["max"], 4.);
    }
}
