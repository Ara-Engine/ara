use crate::{Frame, Profile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Nearest-rank frame-level distribution, including zero for inactive frames.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Distribution {
    /// Median nanoseconds.
    pub p50: u64,
    /// 95th percentile nanoseconds.
    pub p95: u64,
    /// 99th percentile nanoseconds.
    pub p99: u64,
}
impl Distribution {
    pub(crate) fn samples(mut samples: Vec<u64>) -> Self {
        samples.sort_unstable();
        let rank = |n: usize| {
            samples
                .get((samples.len() * n).div_ceil(100).saturating_sub(1))
                .copied()
                .unwrap_or(0)
        };
        Self {
            p50: rank(50),
            p95: rank(95),
            p99: rank(99),
        }
    }
}

/// One CPU system/scope or GPU pass, aggregated by frame rather than invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    /// Stable identity (`cpu/name` or `gpu/queue/name`).
    pub key: String,
    /// Sum of self time for CPU, device duration for GPU.
    pub total_ns: u64,
    /// Per-frame self/device duration distribution.
    pub time: Distribution,
    /// Per-frame inclusive CPU duration distribution (same as time for GPU).
    pub inclusive: Distribution,
    /// CPU bytes allocated excluding nested scopes; zero for GPU passes.
    pub allocated_bytes: u64,
}

/// Over-budget CPU frame or individual GPU pass and its measured contributors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hitch {
    /// Correlated CPU frame ID.
    pub frame: u64,
    /// CPU frame duration.
    pub duration_ns: u64,
    /// Frame budget used by the detector.
    pub budget_ns: u64,
    /// Top CPU scopes and individual GPU passes (never summed across queues).
    pub contributors: Vec<(String, u64)>,
    /// Plain-language evidence, without claiming causality beyond the measurement.
    pub hints: Vec<String>,
}

/// Agent response limits. Hard clamps prevent unbounded responses.
#[derive(Debug, Clone, Copy)]
pub struct SummaryBudget {
    /// Maximum CPU/GPU metric rows, ordered by total self/device time.
    pub top_n: usize,
    /// Maximum hitch records, ordered by worst CPU/GPU duration.
    pub hitches: usize,
    /// Maximum serialized response bytes; must be at least 512.
    pub max_bytes: usize,
}
impl Default for SummaryBudget {
    fn default() -> Self {
        Self {
            top_n: 20,
            hitches: 5,
            max_bytes: 16_384,
        }
    }
}

/// Budgeted agent report and saved baseline format. Full captures retain all data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    /// Summary format version.
    pub version: u32,
    /// Retained frame count; zero cannot pass regression comparison.
    pub frames: usize,
    /// CPU frame budget.
    pub budget_ns: u64,
    /// CPU wall-frame distribution.
    pub cpu_frame: Distribution,
    /// Top per-system and per-pass distributions.
    pub metrics: Vec<Metric>,
    /// Over-budget frames with evidence and suggestions.
    pub hitches: Vec<Hitch>,
    /// Total number of hitches before response truncation.
    pub hitch_count: usize,
    /// Lost records and orphaned delayed results.
    pub dropped: u64,
    /// True when metric/hitch rows were removed to meet the response budget.
    pub truncated: bool,
    /// Whether metric rows were omitted; such reports cannot serve as baselines.
    pub metrics_truncated: bool,
    /// Maximum measured competing-process CPU fraction, absent without OS samples.
    pub background_cpu: Option<f64>,
}

fn cpu_key(profile: &Profile, label: u32) -> String {
    format!(
        "cpu/{}",
        profile
            .labels
            .get(label as usize)
            .map_or("<invalid label>", String::as_str)
    )
}
fn gpu_key(profile: &Profile, pass: &crate::GpuPass) -> String {
    let name = |label: u32| {
        profile
            .labels
            .get(label as usize)
            .map_or("<invalid label>", String::as_str)
    };
    format!("gpu/{}/{}", name(pass.queue), name(pass.label))
}
fn contributions(profile: &Profile, frame: &Frame) -> Vec<(String, u64)> {
    let mut rows = BTreeMap::<String, u64>::new();
    for span in &frame.spans {
        *rows.entry(cpu_key(profile, span.label)).or_default() += span.self_ns;
    }
    for pass in &frame.gpu {
        *rows.entry(gpu_key(profile, pass)).or_default() += pass.duration_ns;
    }
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(5);
    rows
}

impl Profile {
    /// Detects CPU wall hitches and individual over-budget GPU passes. No absolute
    /// GPU timestamps or queue critical path are inferred from duration-only data.
    pub fn hitches(&self, limit: usize) -> Vec<Hitch> {
        let mut frames: Vec<_> = self
            .frames
            .iter()
            .filter(|f| {
                f.duration_ns > self.budget_ns
                    || f.gpu.iter().any(|p| p.duration_ns > self.budget_ns)
            })
            .collect();
        frames.sort_by_key(|f| {
            std::cmp::Reverse(
                f.duration_ns
                    .max(f.gpu.iter().map(|p| p.duration_ns).max().unwrap_or(0)),
            )
        });
        frames.into_iter().take(limit.min(1024)).map(|f| {
            let contributors = contributions(self, f);
            let mut hints = Vec::new();
            if let Some((name, time)) = contributors.first() {
                hints.push(format!("Largest measured contributor: {name}, {:.3} ms. Inspect its callers and workload counters.", *time as f64 / 1e6));
            } else { hints.push("No CPU spans or GPU results: attach the frame thread and collect worker results before diagnosing.".into()); }
            if f.duration_ns > self.budget_ns && f.spans.iter().map(|s| s.self_ns).sum::<u64>() < f.duration_ns / 2 {
                hints.push("Much of the wall frame is uninstrumented or waiting; inspect synchronization, I/O and scheduler activity.".into());
            }
            if f.load.background_cpu.is_some_and(|load| load > 0.2) {
                hints.push("Background processes consumed over 20% of machine CPU; repeat interleaved A/B samples under similar load.".into());
            }
            if f.dropped > 0 { hints.push("Capture capacity was exceeded; increase capacities before trusting contributor rankings.".into()); }
            Hitch { frame: f.id, duration_ns: f.duration_ns, budget_ns: self.budget_ns, contributors, hints }
        }).collect()
    }

    /// Computes bounded summaries. CPU percentiles are sums per label per frame;
    /// child time is subtracted, and frames where a label is absent contribute zero.
    pub fn summary(&self, budget: SummaryBudget) -> Result<Summary, &'static str> {
        if !(512..=1_048_576).contains(&budget.max_bytes) {
            return Err("summary byte budget must be 512..=1048576");
        }
        let mut values = BTreeMap::<String, (Vec<u64>, Vec<u64>, u64)>::new();
        for (index, frame) in self.frames.iter().enumerate() {
            for span in &frame.spans {
                let row = values
                    .entry(cpu_key(self, span.label))
                    .or_insert_with(|| (vec![0; self.frames.len()], vec![0; self.frames.len()], 0));
                row.0[index] += span.self_ns;
                row.1[index] += span.duration_ns;
                row.2 += span.self_allocated_bytes;
            }
            for pass in &frame.gpu {
                let row = values
                    .entry(gpu_key(self, pass))
                    .or_insert_with(|| (vec![0; self.frames.len()], vec![0; self.frames.len()], 0));
                row.0[index] += pass.duration_ns;
                row.1[index] += pass.duration_ns;
            }
        }
        let mut metrics: Vec<_> = values
            .into_iter()
            .map(|(key, (samples, inclusive, allocated_bytes))| Metric {
                key,
                total_ns: samples.iter().sum(),
                time: Distribution::samples(samples),
                inclusive: Distribution::samples(inclusive),
                allocated_bytes,
            })
            .collect();
        metrics.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then_with(|| a.key.cmp(&b.key)));
        let truncated = metrics.len() > budget.top_n.min(1024);
        metrics.truncate(budget.top_n.min(1024));
        let hitch_count = self
            .frames
            .iter()
            .filter(|f| {
                f.duration_ns > self.budget_ns
                    || f.gpu.iter().any(|p| p.duration_ns > self.budget_ns)
            })
            .count();
        let mut summary = Summary {
            version: 1,
            frames: self.frames.len(),
            budget_ns: self.budget_ns,
            cpu_frame: Distribution::samples(self.frames.iter().map(|f| f.duration_ns).collect()),
            metrics,
            hitches: self.hitches(budget.hitches),
            hitch_count,
            dropped: self.orphaned_results + self.frames.iter().map(|f| f.dropped).sum::<u64>(),
            truncated: truncated || hitch_count > budget.hitches.min(1024),
            metrics_truncated: truncated,
            background_cpu: self
                .frames
                .iter()
                .filter_map(|f| f.load.background_cpu)
                .max_by(f64::total_cmp),
        };
        loop {
            let bytes = serde_json::to_vec(&summary)
                .map_err(|_| "cannot serialize profile summary")?
                .len();
            if bytes <= budget.max_bytes {
                break;
            }
            summary.truncated = true;
            if summary.hitches.pop().is_none() {
                if summary.metrics.pop().is_none() {
                    return Err("summary budget too small");
                }
                summary.metrics_truncated = true;
            }
        }
        Ok(summary)
    }
}
