//! Observe foreign GPU users; never terminate or reconfigure them.
mod model;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

use anyhow::{Context, Result, bail};
pub(crate) use model::Report;
use model::{Classifier, Snapshot};
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
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct Invalid(String);
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{CODE}: INVALID: {}", self.0)
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
    pub(crate) fn start(wait_for_quiet: bool, path: PathBuf) -> Result<Self> {
        let report = Arc::new(Mutex::new(Report::default()));
        let shared = Arc::clone(&report);
        let evidence = path.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop, stop_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let outcome = (|| -> Result<()> {
                let mut sampler = Sampler::new()?;
                let mut classifier = Classifier::new(std::process::id())?;
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
                    let sample = classifier.push(sampler.sample()?, &mut baseline);
                    quiet = if sample.foreign_percent <= 5.
                        && !sample.foreign.iter().any(|p| p.known)
                    {
                        quiet + 1
                    } else {
                        0
                    };
                    if baseline.samples.len() < 3 {
                        continue;
                    }
                    if !baseline.invalid() || wait_for_quiet && quiet >= 3 {
                        if wait_for_quiet && baseline.invalid() {
                            baseline = Report {
                                sampler: sampler.name().into(),
                                samples: baseline.samples[baseline.samples.len() - 3..].to_vec(),
                                quiet_wait_seconds: started.elapsed().as_secs_f64(),
                                ..Default::default()
                            };
                        }
                        *shared.lock().expect("GPU guard state") = baseline;
                        ready_tx.send(Ok(())).ok();
                        break;
                    }
                    *shared.lock().expect("GPU guard state") = baseline.clone();
                    if !wait_for_quiet {
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
                    let snapshot = sampler.sample()?;
                    let mut report = shared.lock().expect("GPU guard state");
                    let invalid = report.invalid();
                    classifier.push(snapshot, &mut report);
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
                guard.save()?;
                Ok(guard)
            }
            Err(error) => {
                guard.stop_and_join();
                guard.save()?;
                Err(invalid_error(format!(
                    "timed part refused: {error}; close foreign GPU users or use --wait-for-quiet; {}",
                    guard.path.display()
                )))
            }
        }
    }
    pub(crate) fn snapshot(&self) -> Report {
        self.report.lock().expect("GPU guard state").clone()
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

fn persist(path: &Path, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(
            &serde_json::json!({"format":"ara.gpu-load/1","status":report.status(),"diagnostic":if report.invalid(){Some(CODE)}else{None},"threshold_percent":5.,"sustained_samples":3,"baseline_seconds":3,"reading":report}),
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

enum Sampler {
    #[cfg(windows)]
    Windows(windows::Sampler),
    Fixture(Vec<Snapshot>),
}
impl Sampler {
    fn new() -> Result<Self> {
        // Explicit test input is always labelled in evidence. It is not a native reading.
        if let Some(path) = std::env::var_os("ARA_DEV_GPU_GUARD_FIXTURE") {
            return Ok(Self::Fixture(serde_json::from_slice(&fs::read(path)?)?));
        }
        #[cfg(windows)]
        {
            Ok(Self::Windows(windows::Sampler::new()?))
        }
        #[cfg(not(windows))]
        {
            bail!("native per-process GPU guard is only available on Windows")
        }
    }
    fn name(&self) -> &str {
        match self {
            #[cfg(windows)]
            Self::Windows(_) => "windows_pdh",
            Self::Fixture(_) => "test_fixture",
        }
    }
    fn sample(&mut self) -> Result<Snapshot> {
        match self {
            #[cfg(windows)]
            Self::Windows(s) => s.sample(),
            Self::Fixture(samples) => {
                if samples.is_empty() {
                    Ok(Snapshot::default())
                } else {
                    Ok(samples.remove(0))
                }
            }
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
