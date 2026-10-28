//! Observe foreign GPU users; never terminate or reconfigure them.
#[cfg(any(target_os = "macos", test))]
mod agx;
mod cpu;
#[cfg(target_os = "macos")]
mod macos;
mod model;
pub(crate) mod monitor;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

use anyhow::{Context, Result, bail};
use model::{Classifier, Sample, Snapshot};
pub(crate) use model::{Interval, Policy, Report};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) const CODE: &str = "ARA_GPU_FOREIGN_LOAD";
pub(crate) const CPU_CODE: &str = "ARA_CPU_FOREIGN_LOAD";
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static CURRENT: Mutex<Option<std::sync::Weak<Mutex<Report>>>> = Mutex::new(None);

/// Live guard evidence for reports published inside an outer D26/GPU lease.
pub(crate) fn reading() -> Option<Report> {
    let shared = CURRENT
        .lock()
        .expect("guard registry")
        .as_ref()?
        .upgrade()?;
    let report = shared.lock().expect("guard evidence").clone();
    Some(report)
}

#[derive(Debug)]
struct Invalid(String);
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = if self.0.contains(CPU_CODE) {
            CPU_CODE
        } else {
            CODE
        };
        write!(f, "{code}: INVALID: {}", self.0)
    }
}
impl std::error::Error for Invalid {}
pub(crate) fn invalid_error(detail: String) -> anyhow::Error {
    Invalid(detail).into()
}
pub(crate) fn is_invalid(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Invalid>().is_some()
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Acquire) > 0
}

/// Own a sampler thread for a reading; dropping it stops sampling, never child work.
pub(crate) struct Guard {
    report: Arc<Mutex<Report>>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
    path: PathBuf,
    counted: bool,
}

impl Guard {
    pub(crate) fn start(wait_for_quiet: bool, path: PathBuf, policy: Policy) -> Result<Self> {
        let policy = policy.validate()?;
        let report = Arc::new(Mutex::new(Report::default()));
        let shared = Arc::clone(&report);
        let evidence = path.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop, stop_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let outcome = (|| -> Result<()> {
                let mut sampler = Sampler::new()?;
                #[cfg(windows)]
                if sampler.name() != "test_fixture" {
                    use windows_sys::Win32::System::Threading::*;
                    // SAFETY: query-only pseudo handle for this process. A
                    // BelowNormal caller needs +2 for a Normal-equivalent sampler.
                    let below = unsafe {
                        GetPriorityClass(GetCurrentProcess()) == BELOW_NORMAL_PRIORITY_CLASS
                    };
                    // SAFETY: only this sampler thread changes, never Cargo's class.
                    let adjusted = unsafe {
                        SetThreadPriority(
                            GetCurrentThread(),
                            if below {
                                THREAD_PRIORITY_HIGHEST
                            } else {
                                THREAD_PRIORITY_NORMAL
                            },
                        )
                    };
                    anyhow::ensure!(
                        adjusted != 0,
                        "load sampler thread priority: {}",
                        std::io::Error::last_os_error()
                    );
                }
                let mut cpu_sampler = if sampler.name() == "test_fixture" {
                    None
                } else {
                    Some(cpu::Sampler::new()?)
                };
                let mut classifier = Classifier::with_policy(std::process::id(), policy)?;
                let started = Instant::now();
                let mut baseline = Report {
                    sampler: sampler.name().into(),
                    ..Default::default()
                };
                let mut quiet = 0;
                loop {
                    if stop_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
                        return Ok(());
                    }
                    let Some(sample) =
                        observe_tick(&mut classifier, &mut baseline, sampler.sample())
                    else {
                        quiet = 0;
                        *shared.lock().expect("GPU guard state") = baseline.clone();
                        if baseline.invalid() && !wait_for_quiet {
                            bail!("{}", baseline.detail());
                        }
                        continue;
                    };
                    sample_cpu(&mut cpu_sampler, &mut baseline, policy);
                    quiet = if sample.foreign_percent <= policy.threshold_percent
                        && !sample.foreign.iter().any(|p| p.known)
                        && (cpu_sampler.is_none() || baseline.cpu.quiet())
                    {
                        quiet + 1
                    } else {
                        0
                    };
                    if baseline.samples.len() < 3 {
                        continue;
                    }
                    if quiet >= 3
                        && ((!baseline.invalid() && !classifier.pending()) || wait_for_quiet)
                    {
                        if wait_for_quiet && baseline.invalid() {
                            baseline = Report {
                                sampler: sampler.name().into(),
                                samples: baseline.samples[baseline.samples.len() - 3..].to_vec(),
                                quiet_wait_seconds: started.elapsed().as_secs_f64(),
                                policy,
                                classification_version: 2,
                                cpu: cpu::Report {
                                    samples: baseline
                                        .cpu
                                        .samples
                                        .iter()
                                        .rev()
                                        .take(3)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .into_iter()
                                        .rev()
                                        .collect(),
                                    threshold_percent: policy.cpu_threshold_percent,
                                    sustained_samples: policy.cpu_sustained_samples,
                                    ..Default::default()
                                },
                                monitoring: monitor::Monitoring {
                                    start_unix_ms: Some(
                                        baseline.samples[baseline.samples.len() - 3]
                                            .interval_start_unix_ms,
                                    ),
                                    end_unix_ms: Some(sample.unix_ms),
                                    ..Default::default()
                                },
                                ..Default::default()
                            };
                        }
                        *shared.lock().expect("GPU guard state") = baseline;
                        ready_tx.send(Ok(())).ok();
                        break;
                    }
                    *shared.lock().expect("GPU guard state") = baseline.clone();
                    if !wait_for_quiet && baseline.invalid() {
                        bail!("{}", baseline.detail());
                    }
                    if baseline.samples.len() % 10 == 3 {
                        eprintln!("{CODE}: waiting for quiet: {}", baseline.detail());
                    }
                }
                loop {
                    let final_sample = match stop_rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                        Err(mpsc::RecvTimeoutError::Timeout) => false,
                    };
                    let tick = sampler.sample();
                    let mut report = shared.lock().expect("GPU guard state");
                    let invalid = report.invalid();
                    observe_tick(&mut classifier, &mut report, tick);
                    sample_cpu(&mut cpu_sampler, &mut report, policy);
                    if report.invalid() && !invalid {
                        eprintln!("{CODE}: INVALID reading: {}", report.detail());
                        persist(&evidence, &report)?;
                    }
                    if final_sample {
                        break;
                    }
                }
                Ok(())
            })();
            if let Err(error) = outcome {
                let detail = format!("{error:#}");
                eprintln!("{CODE}: INVALID GPU monitoring: {detail}");
                shared
                    .lock()
                    .expect("GPU guard state")
                    .errors
                    .push(detail.clone());
                let _ = persist(&evidence, &shared.lock().expect("GPU guard state"));
                ready_tx.send(Err(detail)).ok();
            }
        });
        let mut guard = Self {
            report,
            stop,
            worker: Some(worker),
            path,
            counted: false,
        };
        match ready_rx
            .recv()
            .context("GPU sampler stopped during baseline")?
        {
            Ok(()) => {
                ACTIVE.fetch_add(1, Ordering::Release);
                guard.counted = true;
                *CURRENT.lock().expect("guard registry") = Some(Arc::downgrade(&guard.report));
                guard.save()?;
                Ok(guard)
            }
            Err(error) => {
                guard.stop_and_join();
                guard.save()?;
                Err(invalid_error(format!(
                    "timed part refused: {error}; wait for background CPU/GPU work or use --wait-for-quiet; {}",
                    guard.path.display()
                )))
            }
        }
    }
    pub(crate) fn snapshot(&self) -> Report {
        let mut report = self.report.lock().expect("GPU guard state").clone();
        if ["windows_pdh", "macos_agx"].contains(&report.sampler.as_str()) {
            let last = report.samples.last().map(|sample| sample.unix_ms);
            report.monitoring.finish(unix_ms(), last);
        }
        report
    }
    pub(crate) fn save(&self) -> Result<()> {
        persist(&self.path, &self.snapshot())
    }
    fn stop_and_join(&mut self) {
        self.stop.send(()).ok();
        if let Some(worker) = self.worker.take() {
            worker.join().ok();
        }
    }
    pub(crate) fn finish(mut self) -> Result<Report> {
        self.stop_and_join();
        self.save()?;
        Ok(self.snapshot())
    }
}

fn sample_cpu(sampler: &mut Option<cpu::Sampler>, report: &mut Report, policy: Policy) {
    if let Some(sampler) = sampler {
        match sampler.sample() {
            Ok(sample) => report.cpu.observe(
                sample,
                policy.cpu_threshold_percent,
                policy.cpu_sustained_samples,
            ),
            Err(error) => report.cpu.errors.push(format!("{error:#}")),
        }
    }
}

fn persist(path: &Path, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(
            &serde_json::json!({"format":"ara.gpu-load/1","status":report.status(),"diagnostic":report.diagnostic(),"threshold_percent":report.policy.threshold_percent,"sustained_samples":report.policy.sustained_samples,"dwm_allowance_percent":report.policy.dwm_allowance_percent,"baseline_seconds":3,"reading":report}),
        )?,
    )?;
    Ok(())
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop_and_join();
        if self.counted {
            ACTIVE.fetch_sub(1, Ordering::Release);
        }
        let _ = self.save();
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum FixtureFrame {
    Missing { unix_ms: u64, error: String },
    Sample(Snapshot),
}

enum Sampler {
    #[cfg(target_os = "macos")]
    Mac(macos::Sampler),
    #[cfg(windows)]
    Windows(windows::Sampler),
    Fixture {
        frames: std::collections::VecDeque<FixtureFrame>,
        last_unix_ms: u64,
    },
}
impl Sampler {
    fn new() -> Result<Self> {
        // Explicit test input is always labelled in evidence. It is not a native reading.
        if let Some(path) = std::env::var_os("ARA_DEV_GPU_GUARD_FIXTURE") {
            return Ok(Self::Fixture {
                frames: serde_json::from_slice(&fs::read(path)?)?,
                last_unix_ms: 0,
            });
        }
        #[cfg(windows)]
        {
            Ok(Self::Windows(windows::Sampler::new()?))
        }
        #[cfg(target_os = "macos")]
        {
            Ok(Self::Mac(macos::Sampler::new()?))
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            bail!("native per-process GPU guard is only available on Windows")
        }
    }
    fn name(&self) -> &str {
        match self {
            #[cfg(windows)]
            Self::Windows(_) => "windows_pdh",
            #[cfg(target_os = "macos")]
            Self::Mac(_) => "macos_agx",
            Self::Fixture { .. } => "test_fixture",
        }
    }
    fn sample(&mut self) -> (u64, Result<Snapshot>) {
        match self {
            #[cfg(target_os = "macos")]
            Self::Mac(s) => {
                let sample = s.sample();
                (
                    sample.as_ref().map_or_else(|_| unix_ms(), |s| s.unix_ms),
                    sample,
                )
            }
            #[cfg(windows)]
            Self::Windows(s) => {
                let sample = s.sample();
                (
                    sample
                        .as_ref()
                        .map_or_else(|_| unix_ms(), |sample| sample.unix_ms),
                    sample,
                )
            }
            Self::Fixture {
                frames,
                last_unix_ms,
            } => {
                let frame = frames.pop_front().unwrap_or_else(|| {
                    FixtureFrame::Sample(Snapshot {
                        unix_ms: last_unix_ms.saturating_add(1000),
                        ..Default::default()
                    })
                });
                match frame {
                    FixtureFrame::Missing { unix_ms, error } => {
                        *last_unix_ms = unix_ms;
                        (unix_ms, Err(anyhow::anyhow!(error)))
                    }
                    FixtureFrame::Sample(snapshot) => {
                        *last_unix_ms = snapshot.unix_ms;
                        (snapshot.unix_ms, Ok(snapshot))
                    }
                }
            }
        }
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A read failure is one missed poll, not the end of the monitoring thread.
fn observe_tick(
    classifier: &mut Classifier,
    report: &mut Report,
    (at, outcome): (u64, Result<Snapshot>),
) -> Option<Sample> {
    let last = report.samples.last().map(|sample| sample.unix_ms);
    match outcome {
        Ok(snapshot) => {
            if last.is_some_and(|last| at.saturating_sub(last) >= monitor::LOSS_TIMEOUT_MS) {
                classifier.interrupted();
            }
            report.monitoring.valid(at, last);
            Some(classifier.push(snapshot, report))
        }
        Err(error) => {
            classifier.interrupted();
            report
                .monitoring
                .unavailable(at, last, format!("{error:#}"));
            None
        }
    }
}

pub(crate) fn default_path(root: &Path) -> PathBuf {
    root.join("target/ara-dev/gpu-load").join(format!(
        "{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ))
}
