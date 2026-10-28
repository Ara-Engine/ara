//! Machine-normalized CPU occupancy minus the protected PID/creation-identity tree.
//! Priority does not change accounting: BelowNormal builds remain foreign CPU load.
use super::model::Interval;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Match Mach process CPU clocks to host nanoseconds; callers validate a nonzero denominator.
#[cfg(any(target_os = "macos", test))]
pub(super) fn mach_nanoseconds(ticks: u64, numer: u32, denom: u32) -> u64 {
    (u128::from(ticks) * u128::from(numer) / u128::from(denom)).min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Sample {
    pub interval: Interval,
    pub total_percent: f64,
    pub protected_percent: f64,
    pub foreign_percent: f64,
    pub foreign: Vec<Load>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Load {
    pub pid: u32,
    pub birth: u64,
    pub name: String,
    pub percent: f64,
}
#[derive(Clone, Default, Deserialize)]
pub(crate) struct Report {
    pub samples: Vec<Sample>,
    pub intervals: Vec<Interval>,
    pub threshold_percent: f64,
    pub sustained_samples: u32,
    pub errors: Vec<String>,
}
impl Serialize for Report {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let count = self.samples.len();
        let mean = (!self.samples.is_empty())
            .then(|| self.samples.iter().map(|s| s.foreign_percent).sum::<f64>() / count as f64);
        let peak = self
            .samples
            .iter()
            .map(|s| s.foreign_percent)
            .reduce(f64::max);
        let deviation = mean.map(|mean| {
            (self
                .samples
                .iter()
                .map(|s| (s.foreign_percent - mean).powi(2))
                .sum::<f64>()
                / count as f64)
                .sqrt()
        });
        let mut out = serializer.serialize_struct("CpuLoad", 6)?;
        out.serialize_field("samples", &self.samples)?;
        out.serialize_field("intervals", &self.intervals)?;
        out.serialize_field("threshold_percent", &self.threshold_percent)?;
        out.serialize_field("sustained_samples", &self.sustained_samples)?;
        out.serialize_field("errors", &self.errors)?;
        out.serialize_field("summary",&serde_json::json!({"status":if count==0{"not_measured"}else if self.invalid(){"invalid"}else{"within_configured_threshold"},"samples":count,"foreign_mean_percent":mean,"foreign_peak_percent":peak,"foreign_standard_deviation_percent":deviation,"above_default_10_percent":peak.is_some_and(|p|p>10.),"priority_independent":true}))?;
        out.end()
    }
}
impl Report {
    pub fn observe(&mut self, sample: Sample, threshold: f64, sustained: u32) {
        self.threshold_percent = threshold;
        self.sustained_samples = sustained;
        self.samples.push(sample);
        let high = self
            .samples
            .iter()
            .rev()
            .take_while(|s| s.foreign_percent > threshold)
            .take(sustained as usize)
            .collect::<Vec<_>>();
        if high.len() == sustained as usize {
            let interval = Interval {
                start_unix_ms: high[high.len() - 1].interval.start_unix_ms,
                end_unix_ms: high[0].interval.end_unix_ms,
            };
            if let Some(last) = self.intervals.last_mut()
                && last.overlaps(interval)
            {
                last.end_unix_ms = interval.end_unix_ms;
            } else {
                self.intervals.push(interval);
            }
        }
    }
    pub fn quiet(&self) -> bool {
        self.samples
            .last()
            .is_some_and(|s| s.foreign_percent <= self.threshold_percent)
    }
    pub fn invalid(&self) -> bool {
        !self.intervals.is_empty() || !self.errors.is_empty()
    }
    pub fn overlaps(&self, window: Interval) -> bool {
        self.intervals.iter().any(|s| s.overlaps(window))
    }
    pub fn detail(&self) -> String {
        let peak = self
            .samples
            .iter()
            .map(|s| s.foreign_percent)
            .fold(0., f64::max);
        format!(
            "ARA_CPU_FOREIGN_LOAD: foreign CPU peak {peak:.2}% (threshold {:.2}%, {} samples); {}",
            self.threshold_percent,
            self.sustained_samples,
            self.errors.join("; ")
        )
    }
}

pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub birth: u64,
    pub name: String,
    pub ticks: u64,
}
pub(super) struct Raw {
    pub at: u64,
    pub total: u64,
    pub idle: u64,
    pub processes: Vec<Process>,
}
pub(super) struct Sampler {
    previous: Raw,
    owned: BTreeMap<u32, u64>,
    root: u32,
}
impl Sampler {
    pub fn new() -> Result<Self> {
        Ok(Self {
            previous: native()?,
            owned: Default::default(),
            root: std::process::id(),
        })
    }
    pub fn sample(&mut self) -> Result<Sample> {
        let current = native()?;
        self.between(current)
    }
    fn between(&mut self, current: Raw) -> Result<Sample> {
        let processes: BTreeMap<_, _> = current.processes.iter().map(|p| (p.pid, p)).collect();
        self.owned
            .retain(|pid, birth| processes.get(pid).is_none_or(|p| p.birth == *birth));
        if let Some(p) = processes.get(&self.root) {
            self.owned.insert(p.pid, p.birth);
        }
        loop {
            let before = self.owned.len();
            for p in processes.values() {
                if self
                    .owned
                    .get(&p.parent)
                    .is_some_and(|birth| *birth <= p.birth)
                {
                    self.owned.insert(p.pid, p.birth);
                }
            }
            if self.owned.len() == before {
                break;
            }
        }
        let previous: BTreeMap<_, _> = self
            .previous
            .processes
            .iter()
            .map(|p| ((p.pid, p.birth), p.ticks))
            .collect();
        let total = current.total.saturating_sub(self.previous.total);
        ensure!(total > 0, "CPU system clock did not advance");
        let total_percent = (100.
            * (total.saturating_sub(current.idle.saturating_sub(self.previous.idle))) as f64
            / total as f64)
            .clamp(0., 100.);
        let mut protected = 0.;
        let mut foreign = Vec::new();
        for p in &current.processes {
            let before = previous.get(&(p.pid, p.birth)).copied().unwrap_or_else(|| {
                // A process born inside this interval has no earlier CPU time.
                // Older unseen processes stay conservative rather than charging
                // their entire lifetime to one sample.
                if birth_ms(p.birth) >= self.previous.at {
                    0
                } else {
                    p.ticks
                }
            });
            let percent = 100. * p.ticks.saturating_sub(before) as f64 / total as f64;
            if self.owned.get(&p.pid) == Some(&p.birth) {
                protected += percent;
            } else if percent > 0.01 {
                foreign.push(Load {
                    pid: p.pid,
                    birth: p.birth,
                    name: p.name.clone(),
                    percent,
                });
            }
        }
        foreign.sort_by(|a, b| b.percent.total_cmp(&a.percent));
        foreign.truncate(16);
        let result = Sample {
            interval: Interval {
                start_unix_ms: self.previous.at,
                end_unix_ms: current.at,
            },
            total_percent,
            protected_percent: protected,
            foreign_percent: (total_percent - protected).max(0.),
            foreign,
        };
        self.previous = current;
        Ok(result)
    }
}

#[cfg(windows)]
fn birth_ms(birth: u64) -> u64 {
    birth.saturating_sub(116_444_736_000_000_000) / 10_000
}
#[cfg(not(windows))]
fn birth_ms(birth: u64) -> u64 {
    birth
}

#[cfg(windows)]
fn native() -> Result<Raw> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{
            GetProcessTimes, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let (mut idle, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: three distinct initialized FILETIME outputs, no process mutation.
    let sampled = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    ensure!(
        sampled != 0,
        "GetSystemTimes failed: {}",
        std::io::Error::last_os_error()
    );
    let mut processes = Vec::new();
    for p in super::windows::processes()? {
        // SAFETY: read-only process query; a disappearing/inaccessible process is conservative foreign load.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, p.pid) };
        if handle.is_null() {
            continue;
        }
        let (mut birth, mut exit, mut pk, mut pu) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: live query handle and four distinct writable FILETIME outputs.
        let read = unsafe { GetProcessTimes(handle, &mut birth, &mut exit, &mut pk, &mut pu) };
        // SAFETY: owns this query-only handle; it is no longer used.
        unsafe { CloseHandle(handle) };
        if read != 0 {
            processes.push(Process {
                pid: p.pid,
                parent: p.parent,
                birth: ticks(birth),
                name: p.name,
                ticks: ticks(pk) + ticks(pu),
            });
        }
    }
    Ok(Raw {
        at: super::unix_ms(),
        total: ticks(kernel) + ticks(user),
        idle: ticks(idle),
        processes,
    })
}
#[cfg(target_os = "macos")]
fn native() -> Result<Raw> {
    super::macos::cpu()
}
#[cfg(not(any(windows, target_os = "macos")))]
fn native() -> Result<Raw> {
    anyhow::bail!("native CPU load sampling is not implemented for this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mach_process_ticks_use_the_host_nanosecond_unit() {
        assert_eq!(mach_nanoseconds(24_000_000, 125, 3), 1_000_000_000);
        assert_eq!(mach_nanoseconds(987_654, 1, 1), 987_654);
        assert_eq!(mach_nanoseconds(u64::MAX, 125, 3), u64::MAX);
    }
    #[test]
    fn subtracts_only_the_protected_tree_and_handles_new_children_and_pid_reuse() {
        let birth = |ms| {
            if cfg!(windows) {
                116_444_736_000_000_000 + ms * 10_000
            } else {
                ms
            }
        };
        let p = |pid, parent, ms, name: &str, ticks| Process {
            pid,
            parent,
            birth: birth(ms),
            name: name.into(),
            ticks,
        };
        let mut sampler = Sampler {
            previous: Raw {
                at: 1000,
                total: 1000,
                idle: 300,
                processes: vec![
                    p(1, 0, 1, "lab", 100),
                    p(2, 1, 2, "player", 100),
                    p(9, 0, 3, "cargo BelowNormal", 100),
                ],
            },
            owned: Default::default(),
            root: 1,
        };
        let sample = sampler
            .between(Raw {
                at: 2000,
                total: 2000,
                idle: 600,
                processes: vec![
                    p(1, 0, 1, "lab", 150),
                    p(2, 1, 2, "player", 250),
                    p(3, 1, 1500, "new player", 100),
                    p(9, 0, 3, "cargo BelowNormal", 500),
                ],
            })
            .unwrap();
        assert_eq!(sample.total_percent, 70.);
        assert_eq!(sample.protected_percent, 30.);
        assert_eq!(sample.foreign_percent, 40.);
        assert_eq!(sample.foreign[0].name, "cargo BelowNormal");
        let sample = sampler
            .between(Raw {
                at: 3000,
                total: 3000,
                idle: 900,
                processes: vec![
                    p(1, 0, 1, "lab", 150),
                    p(2, 0, 2200, "reused foreign PID", 300),
                ],
            })
            .unwrap();
        assert_eq!(sample.protected_percent, 0.);
        assert_eq!(sample.foreign_percent, 70.);
    }
    #[test]
    fn short_spikes_do_not_join_separate_bursts_and_background_is_retained() {
        let mut r = Report::default();
        for (i, p) in [20., 20., 0., 20., 20., 20., 0.].into_iter().enumerate() {
            r.observe(
                Sample {
                    interval: Interval {
                        start_unix_ms: i as u64 * 1000,
                        end_unix_ms: (i as u64 + 1) * 1000,
                    },
                    total_percent: p + 5.,
                    protected_percent: 5.,
                    foreign_percent: p,
                    foreign: vec![],
                },
                10.,
                3,
            );
        }
        assert_eq!(
            r.intervals,
            vec![Interval {
                start_unix_ms: 3000,
                end_unix_ms: 6000
            }]
        );
        assert!(r.invalid());
        assert!(r.quiet());
    }
}
