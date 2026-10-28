#![warn(missing_docs)]
//! Common perf-pass convention: Divan kernels plus reproducible, interleaved
//! scene scenarios. Setup/warmup and OS sampling are outside measured frames.
use ara_profile::{BackgroundLoad, Counters, Memory, Profiler};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub mod passes;

/// OS sampler used by every scenario and overhead run. Does not spawn a sampler
/// thread or wait; intervals shorter than the OS minimum are marked unavailable.
pub struct LoadSampler {
    system: System,
    previous: Instant,
    previous_cpu_ms: Option<u64>,
    pid: sysinfo::Pid,
}
impl Default for LoadSampler {
    fn default() -> Self {
        Self::new()
    }
}
impl LoadSampler {
    /// Primes cumulative process/system CPU counters, outside measurement.
    pub fn new() -> Self {
        let mut sampler = Self {
            system: System::new(),
            previous: Instant::now(),
            previous_cpu_ms: None,
            pid: sysinfo::Pid::from_u32(std::process::id()),
        };
        sampler.refresh();
        sampler.previous_cpu_ms = sampler
            .system
            .process(sampler.pid)
            .map(|p| p.accumulated_cpu_time());
        sampler.previous = Instant::now();
        sampler
    }
    fn refresh(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
    }
    /// Samples only after the OS minimum interval; missing data stays `None`.
    pub fn sample(&mut self) -> BackgroundLoad {
        if self.previous.elapsed() < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
            return BackgroundLoad::default();
        }
        self.refresh();
        let now = Instant::now();
        let elapsed = now.duration_since(self.previous).as_secs_f64();
        self.previous = now;
        if self.system.cpus().is_empty() {
            return BackgroundLoad::default();
        }
        let total = f64::from(self.system.global_cpu_usage()) / 100.0;
        let cpu_ms = self
            .system
            .process(self.pid)
            .map(|p| p.accumulated_cpu_time());
        // Use cumulative CPU milliseconds over our measured interval. This keeps
        // process and system samples on the same interval on every OS backend.
        let process = self
            .previous_cpu_ms
            .zip(cpu_ms)
            .and_then(|(before, after)| after.checked_sub(before))
            .map(|delta| {
                (delta as f64 / (elapsed * 1000.0 * self.system.cpus().len() as f64))
                    .clamp(0.0, 1.0)
            });
        self.previous_cpu_ms = cpu_ms;
        BackgroundLoad {
            total_cpu: Some(total.clamp(0.0, 1.0)),
            process_cpu: process,
            background_cpu: process.map(|process| (total - process).clamp(0.0, 1.0)),
        }
    }
}

/// A reproducible scene workload. Own all simulation state inside its ECS world.
pub trait Scenario {
    /// Reset/load the same scene and seed before each A/B round (not timed).
    fn reset(&mut self) -> Result<(), String>;
    /// Run exactly one fixed tick or presentation frame (timed).
    fn frame(&mut self) -> Result<Counters, String>;
    /// Memory owned by this scenario, if available (sampled after frame work).
    fn memory(&self) -> Memory {
        Memory::default()
    }
}

/// Scripted run parameters, usable from canonical JSON scenario manifests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScenarioConfig {
    /// Untimed frames after each reset.
    pub warmup: u32,
    /// Measured frames per variant per round.
    pub frames: u32,
    /// Number of A/B rounds. Odd rounds reverse order (AB, BA, AB, BA).
    pub rounds: u32,
}

/// One paired round's wall time, kept in execution order for load analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSample {
    /// First measured frame ID in the shared profile.
    pub first_frame: u64,
    /// Last measured frame ID (inclusive).
    pub last_frame: u64,
    /// Zero-based round identity.
    pub round: u32,
    /// `a` or `b`.
    pub variant: String,
    /// Measured frame count.
    pub frames: u32,
    /// Measured frame wall time, excluding load sampling and capture serialization.
    pub duration_ns: u64,
    /// System and competing-process CPU load over this round when available.
    pub load: BackgroundLoad,
}

/// Runs matched AB/BA rounds on one attached, recording profiler. Split its
/// retained frames by the returned inclusive ranges to save separate baselines.
/// Size history for all rounds if both variants' full traces must be retained.
pub fn interleave(
    a: &mut impl Scenario,
    b: &mut impl Scenario,
    profiler: &mut Profiler,
    config: ScenarioConfig,
) -> Result<Vec<RunSample>, String> {
    if config.frames == 0 || config.rounds == 0 {
        return Err("scenario frames and rounds must be nonzero".into());
    }
    let mut load = LoadSampler::new();
    let mut samples = Vec::with_capacity(config.rounds as usize * 2);
    for round in 0..config.rounds {
        for variant in if round % 2 == 0 {
            ["a", "b"]
        } else {
            ["b", "a"]
        } {
            let scenario: &mut dyn Scenario = if variant == "a" { a } else { b };
            scenario.reset()?;
            for _ in 0..config.warmup {
                scenario.frame()?;
            }
            let mut duration_ns = 0;
            let mut ids = Vec::with_capacity(config.frames as usize);
            for _ in 0..config.frames {
                let frame = profiler.begin_frame().map_err(str::to_owned)?;
                let counters = match scenario.frame() {
                    Ok(counters) => counters,
                    Err(error) => {
                        let _ = profiler.end_frame(Counters::default(), Memory::default());
                        return Err(error);
                    }
                };
                profiler
                    .end_frame(counters, scenario.memory())
                    .map_err(str::to_owned)?;
                if let Some(record) = profiler.profile().frames.back() {
                    duration_ns += record.duration_ns;
                }
                ids.push(frame.id());
            }
            let background = load.sample();
            for &id in &ids {
                profiler.set_load(id, background);
            }
            samples.push(RunSample {
                first_frame: ids.first().copied().unwrap_or(0),
                last_frame: ids.last().copied().unwrap_or(0),
                round,
                variant: variant.into(),
                frames: config.frames,
                duration_ns,
                load: background,
            });
        }
    }
    Ok(samples)
}
