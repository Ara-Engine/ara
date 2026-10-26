use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    pub foreign_percent: f64,
    pub foreign: Vec<Load>,
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
}

impl Report {
    pub(crate) fn invalid(&self) -> bool {
        !self.contaminants.is_empty() || !self.errors.is_empty()
    }
    pub(crate) fn status(&self) -> &'static str {
        if self.invalid() {
            "invalid"
        } else if self.sampler == "test_fixture" {
            "test_only"
        } else {
            "valid"
        }
    }
    pub(crate) fn detail(&self) -> String {
        let mut notes: Vec<_> = self
            .contaminants
            .iter()
            .map(|p| {
                format!(
                    "{} (PID {}, peak {:.2}%, {}..{} UTC epoch ms, {})",
                    p.name, p.pid, p.peak_percent, p.first_unix_ms, p.last_unix_ms, p.reason
                )
            })
            .collect();
        notes.extend(self.errors.clone());
        notes.join("; ")
    }
    pub(super) fn note(&mut self, sample: &Sample, load: &Load, reason: &str) {
        if let Some(p) = self
            .contaminants
            .iter_mut()
            .find(|p| p.pid == load.pid && p.birth == load.birth)
        {
            p.first_unix_ms = p.first_unix_ms.min(sample.unix_ms);
            p.last_unix_ms = p.last_unix_ms.max(sample.unix_ms);
            p.peak_percent = p.peak_percent.max(load.percent);
        } else {
            self.contaminants.push(Incident {
                pid: load.pid,
                name: load.name.clone(),
                birth: load.birth,
                peak_percent: load.percent,
                first_unix_ms: sample.unix_ms,
                last_unix_ms: sample.unix_ms,
                reason: reason.into(),
            });
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Rules {
    presence: Vec<String>,
}

pub(super) struct Classifier {
    root: u32,
    owned: BTreeMap<u32, u64>,
    rules: Rules,
    high: Vec<Sample>,
}

pub(super) fn counter_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("pid_")?;
    rest.split('_').next()?.parse().ok()
}

impl Classifier {
    pub fn new(root: u32) -> anyhow::Result<Self> {
        Ok(Self {
            root,
            owned: BTreeMap::new(),
            rules: toml_edit::de::from_str(include_str!("../../gpu-processes.toml"))?,
            high: Vec::new(),
        })
    }
    pub fn push(&mut self, snapshot: Snapshot, report: &mut Report) -> Sample {
        let processes: BTreeMap<_, _> =
            snapshot.processes.into_iter().map(|p| (p.pid, p)).collect();
        // PDH retains a child's final interval after exit. Preserve that identity
        // until a live snapshot proves PID reuse, so its tail is not foreign load.
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
        for (instance, percent) in snapshot.engines {
            if let Some(pid) = counter_pid(&instance) {
                if !percent.is_finite() || percent < 0. {
                    report
                        .errors
                        .push(format!("invalid GPU counter for PID {pid}"));
                    continue;
                }
                if !self.owned.contains_key(&pid) {
                    *values.entry(pid).or_default() += percent;
                }
            }
        }
        let mut foreign = Vec::new();
        for p in processes
            .values()
            .filter(|p| !self.owned.contains_key(&p.pid))
        {
            let known = self
                .rules
                .presence
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&p.name));
            let percent = values.remove(&p.pid).unwrap_or(0.);
            if known || percent > 0. {
                foreign.push(Load {
                    pid: p.pid,
                    name: p.name.clone(),
                    birth: p.birth,
                    percent,
                    known,
                });
            }
        }
        for (pid, percent) in values {
            if percent > 0. {
                foreign.push(Load {
                    pid,
                    name: "unknown/exited process".into(),
                    birth: 0,
                    percent,
                    known: false,
                });
            }
        }
        let sample = Sample {
            unix_ms: snapshot.unix_ms,
            foreign_percent: foreign.iter().map(|p| p.percent).sum(),
            foreign,
        };
        for load in sample.foreign.iter().filter(|p| p.known) {
            report.note(&sample, load, "known game executable present");
        }
        if sample.foreign_percent > 5. {
            self.high.push(sample.clone());
            if self.high.len() >= 3 {
                for high in &self.high {
                    for load in &high.foreign {
                        report.note(high, load, "foreign GPU sum >5% for at least 3 samples");
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
