//! Opt-in frame windows and a file control queue; ordinary exported games retain their CLI behavior.
use anyhow::{Context, Result, ensure};
use ara_gpu::vulkan::PassTiming;
use ara_runtime_render::Presenter;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod gpu;

#[derive(clap::Args)]
pub(super) struct PerfOptions {
    /// Publish detailed frame-window metrics to this JSON file (development only).
    #[arg(long)]
    perf_output: Option<PathBuf>,
    #[arg(long, default_value_t = 240, value_parser = clap::value_parser!(u32).range(1..))]
    perf_frames: u32,
    #[arg(long, default_value_t = 30)]
    perf_warmup: u32,
    /// JSON request queue: quality, reload_shaders, measure and stop.
    #[arg(long, requires = "perf_output")]
    perf_control: Option<PathBuf>,
    #[arg(long, requires = "perf_output")]
    keep_alive: bool,
}

#[derive(Default, Deserialize)]
struct Control {
    quality: Option<String>,
    #[serde(default)]
    reload_shaders: bool,
    #[serde(default)]
    measure: bool,
    #[serde(default)]
    stop: bool,
}

pub(super) struct Window {
    options: PerfOptions,
    frames: BTreeMap<String, Vec<f64>>,
    passes: BTreeMap<String, Vec<f64>>,
    gpu: gpu::Samples,
    seen: u32,
    measured: u32,
    collecting: bool,
    next_control: Instant,
    waiting_for_shader_epoch: Option<u64>,
    last_epoch: u64,
    live_status: Vec<u8>,
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
    json!({"p50":rank(0.5),"p95":rank(0.95),"max":sorted[sorted.len()-1],"mean":sorted.iter().sum::<f64>()/sorted.len() as f64,"samples":sorted.len()})
}

impl Window {
    pub fn enabled(&self) -> bool {
        self.options.perf_output.is_some()
    }
    pub fn new(options: PerfOptions) -> Self {
        let collecting = options.perf_output.is_some();
        Self {
            options,
            frames: BTreeMap::new(),
            passes: BTreeMap::new(),
            gpu: gpu::Samples::default(),
            seen: 0,
            measured: 0,
            collecting,
            next_control: Instant::now(),
            waiting_for_shader_epoch: None,
            last_epoch: 0,
            live_status: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.frames.clear();
        self.passes.clear();
        self.gpu = gpu::Samples::default();
        self.seen = 0;
        self.measured = 0;
        self.collecting = true;
    }

    pub fn control(&mut self, presenter: &mut Presenter) -> Result<bool> {
        if self.options.perf_output.is_none() || Instant::now() < self.next_control {
            return Ok(false);
        }
        self.next_control = Instant::now() + Duration::from_millis(100);
        let shaders = presenter.shader_status();
        if shaders.epoch != self.last_epoch && !shaders.pending && shaders.last_error.is_none() {
            self.last_epoch = shaders.epoch;
            self.reset();
        }
        if let Some(output) = &self.options.perf_output {
            if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let status = serde_json::to_vec_pretty(
                &json!({"quality":presenter.quality().map(|q| q.name()),"collecting":self.collecting,"shaders":shaders}),
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
            if command.stop {
                std::fs::remove_file(request)?;
                return Ok(true);
            }
            if let Some(quality) = &command.quality {
                presenter.set_quality(quality.parse()?);
            }
            if command.reload_shaders {
                self.waiting_for_shader_epoch = Some(presenter.shader_status().epoch);
                presenter.reload_shaders();
            }
            if command.measure || command.quality.is_some() || command.reload_shaders {
                self.reset();
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
        cpu: [f64; 4],
        total: f64,
    ) -> Result<bool> {
        if !self.collecting {
            return Ok(false);
        }
        if let Some(epoch) = self.waiting_for_shader_epoch {
            let status = presenter.shader_status();
            if status.pending {
                return Ok(false);
            }
            if let Some(error) = status.last_error {
                eprintln!("Shader reload failed; previous result retained: {error}");
                self.collecting = false;
                self.waiting_for_shader_epoch = None;
                return Ok(false);
            }
            if status.epoch == epoch {
                return Ok(false);
            }
            self.waiting_for_shader_epoch = None;
        }
        self.seen += 1;
        if self.seen <= self.options.perf_warmup {
            return Ok(false);
        }
        for (name, value) in [
            "simulation_and_extraction",
            "presentation_prepare",
            "presentation_including_display",
            "audio",
        ]
        .into_iter()
        .zip(cpu)
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
        let output = self
            .options
            .perf_output
            .as_ref()
            .context("metrics enabled")?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cpu: BTreeMap<_, _> = self
            .frames
            .iter()
            .map(|(name, samples)| (name, quantiles(samples)))
            .collect();
        let passes: BTreeMap<_, _> = self
            .passes
            .iter()
            .map(|(name, samples)| (name, quantiles(samples)))
            .collect();
        ensure!(
            !passes.is_empty(),
            "perf window did not receive GPU timestamp samples"
        );
        let report = json!({"format":"ara.player.perf/1","pid":std::process::id(),"quality":presenter.quality().map(|q|q.name()),"validation":cfg!(debug_assertions),"startup":phases,"camera":camera,"frames":self.measured,"warmup":self.options.perf_warmup,"cpu":cpu,"gpu_passes":passes,"gpu_other":self.gpu.report(),"gpu_coverage":self.gpu.coverage(),"shaders":presenter.shader_status()});
        let temporary = output.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&report)?)?;
        std::fs::rename(temporary, output)?;
        eprintln!("Perf window published: {}", output.display());
        self.collecting = false;
        Ok(!self.options.keep_alive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_real_quantiles_and_keeps_no_samples_missing() {
        assert!(quantiles(&[]).is_null());
        let stats = quantiles(&[4., 1., 2., 3.]);
        assert_eq!(stats["p50"], 2.);
        assert_eq!(stats["p95"], 4.);
        assert_eq!(stats["max"], 4.);
    }
}
