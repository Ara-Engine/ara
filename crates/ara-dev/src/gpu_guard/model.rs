//! Pure policy and interval classification, shared by live and saved readings.
use anyhow::{Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn percent(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| "expected a percentage".to_owned())?;
    if value.is_finite() && (0. ..=1000.).contains(&value) {
        Ok(value)
    } else {
        Err("percentage must be finite and between 0 and 1000".into())
    }
}

/// Desktop work is charged after a small compositor allowance; services stay measured.
#[derive(Args, Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub(crate) struct Policy {
    /// Foreign GPU sum above this percentage qualifies after the sustained sample count.
    #[arg(long="foreign-gpu-percent",default_value_t=10.,value_parser=percent)]
    pub threshold_percent: f64,
    /// Consecutive samples above the foreign threshold before invalidation.
    #[arg(long="foreign-gpu-samples",default_value_t=5,value_parser=clap::value_parser!(u32).range(1..=3600))]
    pub sustained_samples: u32,
    /// DWM's allowance for the reading's own presentation; only excess is foreign.
    #[arg(long="dwm-allowance-percent",default_value_t=3.,value_parser=percent)]
    pub dwm_allowance_percent: f64,
    /// Foreign CPU occupancy, normalized over all logical processors, including BelowNormal work.
    #[arg(long="foreign-cpu-percent",default_value_t=10.,value_parser=percent)]
    pub cpu_threshold_percent: f64,
    /// Consecutive one-second CPU samples above threshold before a reading is invalid.
    #[arg(long="foreign-cpu-samples",default_value_t=3,value_parser=clap::value_parser!(u32).range(1..=3600))]
    pub cpu_sustained_samples: u32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            threshold_percent: 10.,
            sustained_samples: 5,
            dwm_allowance_percent: 3.,
            cpu_threshold_percent: 10.,
            cpu_sustained_samples: 3,
        }
    }
}
impl Policy {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            self.cpu_threshold_percent.is_finite()
                && (0. ..=100.).contains(&self.cpu_threshold_percent),
            "invalid foreign CPU threshold (0..100)"
        );
        ensure!(
            (1..=3600).contains(&self.cpu_sustained_samples),
            "invalid sustained CPU sample count"
        );
        ensure!(
            self.threshold_percent.is_finite() && self.threshold_percent >= 0.,
            "invalid foreign GPU threshold"
        );
        ensure!(
            self.dwm_allowance_percent.is_finite() && self.dwm_allowance_percent >= 0.,
            "invalid DWM allowance"
        );
        ensure!(
            (1..=3600).contains(&self.sustained_samples),
            "invalid sustained GPU sample count"
        );
        Ok(self)
    }
}

/// Inclusive UTC interval, kept separate from per-process aggregate peaks.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Interval {
    pub start_unix_ms: u64,
    pub end_unix_ms: u64,
}
impl Interval {
    pub fn overlaps(self, other: Self) -> bool {
        self.start_unix_ms <= other.end_unix_ms && other.start_unix_ms <= self.end_unix_ms
    }
}

#[derive(Clone, Deserialize)]
pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub birth: u64,
}
#[derive(Default, Deserialize)]
pub(super) struct Snapshot {
    pub unix_ms: u64,
    pub processes: Vec<Process>,
    pub engines: Vec<(String, f64)>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Load {
    pub pid: u32,
    pub name: String,
    pub birth: u64,
    pub percent: f64,
    pub known: bool,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Sample {
    pub unix_ms: u64,
    #[serde(default)]
    pub interval_start_unix_ms: u64,
    pub foreign_percent: f64,
    pub foreign: Vec<Load>,
    /// Unmodified process counters support reclassification with another allowance.
    #[serde(default)]
    pub raw: Vec<Load>,
    /// Kernel/CSRSS and full DWM activity, for information rather than threshold charging.
    #[serde(default)]
    pub informational: Vec<Load>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Incident {
    pub pid: u32,
    pub name: String,
    pub birth: u64,
    pub peak_percent: f64,
    pub first_unix_ms: u64,
    pub last_unix_ms: u64,
    pub reason: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct Report {
    pub sampler: String,
    pub samples: Vec<Sample>,
    pub contaminants: Vec<Incident>,
    pub errors: Vec<String>,
    pub quiet_wait_seconds: f64,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub intervals: Vec<Interval>,
    #[serde(default)]
    pub classification_version: u32,
    #[serde(default)]
    pub monitoring: super::monitor::Monitoring,
    #[serde(default)]
    pub cpu: super::cpu::Report,
}
impl Report {
    pub fn diagnostic(&self) -> Option<&'static str> {
        if self.cpu.invalid() {
            Some(super::CPU_CODE)
        } else if self.invalid() {
            Some(super::CODE)
        } else {
            None
        }
    }
    pub fn invalid(&self) -> bool {
        !self.intervals.is_empty()
            || self.cpu.invalid()
            || !self.monitoring.gaps.is_empty()
            || !self.errors.is_empty()
            || self.classification_version == 0 && !self.contaminants.is_empty()
    }
    pub fn status(&self) -> &'static str {
        if self.invalid() {
            "invalid"
        } else if self.sampler == "test_fixture" {
            "test_only"
        } else {
            "valid"
        }
    }
    pub fn overlaps(&self, window: Interval) -> bool {
        self.intervals.iter().any(|span| span.overlaps(window)) || self.cpu.overlaps(window)
    }
    pub fn detail(&self) -> String {
        let mut result: Vec<_> = self
            .contaminants
            .iter()
            .map(|p| {
                format!(
                    "{} (PID {}, peak {:.2}%, {}..{} UTC epoch ms, {})",
                    p.name, p.pid, p.peak_percent, p.first_unix_ms, p.last_unix_ms, p.reason
                )
            })
            .collect();
        result.extend(self.errors.clone());
        if self.cpu.invalid() {
            result.push(self.cpu.detail());
        }
        result.extend(self.monitoring.gaps.iter().map(|gap| {
            format!(
                "unmonitored {}..{} UTC epoch ms: {}",
                gap.interval.start_unix_ms, gap.interval.end_unix_ms, gap.reason
            )
        }));
        result.join("; ")
    }
    fn interval(&mut self, interval: Interval) {
        self.intervals.push(interval);
        self.intervals.sort_by_key(|s| s.start_unix_ms);
        let mut merged: Vec<Interval> = Vec::new();
        for span in self.intervals.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.overlaps(span)
            {
                last.end_unix_ms = last.end_unix_ms.max(span.end_unix_ms);
            } else {
                merged.push(span);
            }
        }
        self.intervals = merged;
    }
    fn note(&mut self, sample: &Sample, load: &Load, reason: &str) {
        let peak = sample
            .raw
            .iter()
            .find(|raw| raw.pid == load.pid && raw.birth == load.birth)
            .map_or(load.percent, |raw| raw.percent);
        if let Some(p) = self
            .contaminants
            .iter_mut()
            .find(|p| p.pid == load.pid && p.birth == load.birth)
        {
            p.first_unix_ms = p.first_unix_ms.min(sample.interval_start_unix_ms);
            p.last_unix_ms = p.last_unix_ms.max(sample.unix_ms);
            p.peak_percent = p.peak_percent.max(peak);
        } else {
            self.contaminants.push(Incident {
                pid: load.pid,
                name: load.name.clone(),
                birth: load.birth,
                peak_percent: peak,
                first_unix_ms: sample.interval_start_unix_ms,
                last_unix_ms: sample.unix_ms,
                reason: reason.into(),
            });
        }
    }
    /// Replay original counters, not the previous charged sums or aggregate incident span.
    pub fn reclassify(&self, policy: Policy) -> Result<Self> {
        ensure!(
            !self.samples.is_empty() || !self.errors.is_empty(),
            "saved GPU reading contains no counter samples"
        );
        let mut classifier = Classifier::with_policy(0, policy)?;
        let mut result = Self {
            sampler: self.sampler.clone(),
            quiet_wait_seconds: self.quiet_wait_seconds,
            errors: self.errors.clone(),
            monitoring: self.monitoring.clone(),
            policy,
            classification_version: 2,
            ..Default::default()
        };
        result.cpu.errors = self.cpu.errors.clone();
        for sample in &self.cpu.samples {
            result.cpu.observe(
                sample.clone(),
                policy.cpu_threshold_percent,
                policy.cpu_sustained_samples,
            );
        }
        for sample in &self.samples {
            classifier.observe(sample.clone(), &mut result);
        }
        Ok(result)
    }

    /// Older guards stopped at their first read error. The lost tail is unknown,
    /// not clean; retain the error as a retry and bound its gap to the saved run.
    pub fn recover_legacy_monitoring(&mut self, end_unix_ms: u64) {
        if self.monitoring.end_unix_ms.is_some() {
            return;
        }
        let Some(first) = self.samples.first() else {
            return;
        };
        self.monitoring.start_unix_ms = Some(if first.interval_start_unix_ms == 0 {
            first.unix_ms.saturating_sub(1000)
        } else {
            first.interval_start_unix_ms
        });
        let last = self.samples.last().map(|sample| sample.unix_ms);
        if !self.errors.is_empty() {
            let reason = self.errors.join("; ");
            self.monitoring.unavailable(
                last.unwrap_or(end_unix_ms)
                    .saturating_add(1000)
                    .min(end_unix_ms),
                last,
                reason,
            );
            self.errors.clear();
        }
        self.monitoring.finish(end_unix_ms, last);
    }
}

#[derive(Deserialize)]
struct Rules {
    presence: Vec<String>,
}
pub(super) struct Classifier {
    root: u32,
    owned: BTreeMap<u32, u64>,
    rules: Rules,
    high: Vec<Sample>,
    policy: Policy,
}
pub(super) fn counter_pid(name: &str) -> Option<u32> {
    name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
}
fn os_path(load: &Load) -> bool {
    load.pid == 4 || load.name.eq_ignore_ascii_case("csrss.exe")
}
fn dwm(load: &Load) -> bool {
    load.name.eq_ignore_ascii_case("dwm.exe")
}

impl Classifier {
    #[cfg(test)]
    pub fn new(root: u32) -> Result<Self> {
        Self::with_policy(root, Policy::default())
    }
    pub fn with_policy(root: u32, policy: Policy) -> Result<Self> {
        Ok(Self {
            root,
            owned: BTreeMap::new(),
            rules: toml_edit::de::from_str(include_str!("../../gpu-processes.toml"))?,
            high: Vec::new(),
            policy: policy.validate()?,
        })
    }
    pub fn pending(&self) -> bool {
        !self.high.is_empty()
    }
    pub fn interrupted(&mut self) {
        self.high.clear();
    }
    pub fn push(&mut self, snapshot: Snapshot, report: &mut Report) -> Sample {
        let processes: BTreeMap<_, _> =
            snapshot.processes.into_iter().map(|p| (p.pid, p)).collect();
        // Retired children can have one last PDH interval; a new birth proves PID reuse.
        self.owned
            .retain(|pid, birth| processes.get(pid).is_none_or(|p| p.birth == *birth));
        if let Some(root) = processes.get(&self.root) {
            self.owned.insert(root.pid, root.birth);
        }
        loop {
            let before = self.owned.len();
            for p in processes.values() {
                if processes.contains_key(&p.parent) && self.owned.contains_key(&p.parent) {
                    self.owned.insert(p.pid, p.birth);
                }
            }
            if before == self.owned.len() {
                break;
            }
        }
        let mut values = BTreeMap::<u32, f64>::new();
        for (name, value) in snapshot.engines {
            if let Some(pid) = counter_pid(&name) {
                if !value.is_finite() || value < 0. {
                    report
                        .errors
                        .push(format!("invalid GPU counter for PID {pid}"));
                } else if !self.owned.contains_key(&pid) {
                    *values.entry(pid).or_default() += value;
                }
            }
        }
        let mut raw = Vec::new();
        for p in processes
            .values()
            .filter(|p| !self.owned.contains_key(&p.pid))
        {
            let known = self
                .rules
                .presence
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&p.name));
            let percent = values.remove(&p.pid).unwrap_or(0.);
            let load = Load {
                pid: p.pid,
                name: p.name.clone(),
                birth: p.birth,
                percent,
                known,
            };
            if known || percent > 0. || os_path(&load) || dwm(&load) {
                raw.push(load);
            }
        }
        for (pid, percent) in values {
            if percent > 0. {
                raw.push(Load {
                    pid,
                    name: "unknown/exited process".into(),
                    birth: 0,
                    percent,
                    known: false,
                });
            }
        }
        self.observe(
            Sample {
                unix_ms: snapshot.unix_ms,
                interval_start_unix_ms: 0,
                foreign_percent: 0.,
                foreign: Vec::new(),
                raw,
                informational: Vec::new(),
            },
            report,
        )
    }
    pub fn observe(&mut self, mut sample: Sample, report: &mut Report) -> Sample {
        report.policy = self.policy;
        report.classification_version = 2;
        if sample.interval_start_unix_ms == 0 {
            sample.interval_start_unix_ms = report
                .samples
                .last()
                .map_or(sample.unix_ms.saturating_sub(1000), |s| s.unix_ms);
        }
        if sample.raw.is_empty() {
            sample.raw = sample.foreign.clone();
        }
        sample.foreign.clear();
        sample.informational.clear();
        for original in &sample.raw {
            let mut load = original.clone();
            load.known = self
                .rules
                .presence
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&load.name));
            if !load.percent.is_finite() || load.percent < 0. {
                report
                    .errors
                    .push(format!("invalid saved GPU counter for PID {}", load.pid));
                continue;
            }
            if os_path(&load) {
                sample.informational.push(load);
                continue;
            }
            if dwm(&load) {
                sample.informational.push(load.clone());
                load.percent = (load.percent - self.policy.dwm_allowance_percent).max(0.);
            }
            if load.percent > 0. || load.known {
                sample.foreign.push(load);
            }
        }
        sample.foreign_percent = sample.foreign.iter().map(|p| p.percent).sum();
        let interval = Interval {
            start_unix_ms: sample.interval_start_unix_ms,
            end_unix_ms: sample.unix_ms,
        };
        for load in sample.foreign.iter().filter(|p| p.known) {
            report.interval(interval);
            report.note(&sample, load, "known game executable present");
        }
        if sample.foreign_percent > self.policy.threshold_percent {
            self.high.push(sample.clone());
            if self.high.len() >= self.policy.sustained_samples as usize {
                report.interval(Interval {
                    start_unix_ms: self.high[0].interval_start_unix_ms,
                    end_unix_ms: sample.unix_ms,
                });
                for high in &self.high {
                    for load in &high.foreign {
                        report.note(
                            high,
                            load,
                            "sustained foreign GPU sum above configured threshold",
                        );
                    }
                }
                self.high.remove(0);
            }
        } else {
            self.high.clear();
        }
        report.samples.push(sample.clone());
        sample
    }
}
