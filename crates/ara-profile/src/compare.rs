use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};
use crate::Summary;

/// One baseline/current p95 comparison row. Missing data is explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    /// CPU frame, system or GPU pass identity.
    pub key: String,
    /// Baseline p95 nanoseconds, absent for a new metric.
    pub baseline_ns: Option<u64>,
    /// Current p95 nanoseconds, absent for a missing metric.
    pub current_ns: Option<u64>,
    /// Fractional change; absent when baseline is zero or either value is missing.
    pub change: Option<f64>,
    /// Above both relative and absolute tolerances, or new/missing data.
    pub regression: bool,
}

/// Local D9 regression verdict; serializable and printable as a delta table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparison {
    /// False for regressions, incomplete captures, truncated summaries or no samples.
    pub passed: bool,
    /// Per-metric p95 changes, including CPU wall-frame time.
    pub deltas: Vec<Delta>,
    /// Reasons the evidence cannot support a passing result.
    pub issues: Vec<String>,
}

/// Compares p95 values. A regression must exceed both `relative` (e.g. 0.05)
/// and `absolute_ns`. New/missing metrics fail instead of disappearing from the table.
pub fn compare(baseline: &Summary, current: &Summary, relative: f64, absolute_ns: u64) -> Result<Comparison, &'static str> {
    if !relative.is_finite() || relative < 0.0 { return Err("relative tolerance must be finite and nonnegative"); }
    let mut issues = Vec::new();
    for (name, summary) in [("baseline", baseline), ("current", current)] {
        if summary.version != 1 { issues.push(format!("{name}: unsupported summary version")); }
        if summary.frames == 0 { issues.push(format!("{name}: no frame samples")); }
        if summary.dropped != 0 { issues.push(format!("{name}: incomplete capture (lost records)")); }
        if summary.metrics_truncated { issues.push(format!("{name}: truncated metrics; save a complete baseline")); }
    }
    if baseline.budget_ns != current.budget_ns { issues.push("frame budgets differ".into()); }
    let names: BTreeSet<_> = baseline.metrics.iter().chain(&current.metrics).map(|m| m.key.as_str()).chain(["frame/cpu"]).collect();
    let lookup = |s: &Summary, key: &str| if key == "frame/cpu" { Some(s.cpu_frame.p95) } else { s.metrics.iter().find(|m| m.key == key).map(|m| m.time.p95) };
    let deltas: Vec<_> = names.into_iter().map(|key| {
        let b = lookup(baseline, key); let c = lookup(current, key);
        let change = b.zip(c).filter(|(b,_)| *b != 0).map(|(b,c)| c as f64 / b as f64 - 1.0);
        let regression = match (b,c) { (Some(b),Some(c)) => c.saturating_sub(b) > absolute_ns && (c as f64) > b as f64 * (1.0 + relative), _ => true };
        Delta { key: key.into(), baseline_ns: b, current_ns: c, change, regression }
    }).collect();
    Ok(Comparison { passed: issues.is_empty() && deltas.iter().all(|d| !d.regression), deltas, issues })
}
impl Comparison {
    /// Human-readable delta table; the CLI returns exit 1 when `passed` is false.
    pub fn table(&self) -> String {
        use std::fmt::Write;
        let mut out = format!("{}\nmetric\tbaseline p95 ms\tcurrent p95 ms\tdelta\n", if self.passed { "PASS" } else { "FAIL" });
        let time = |v: Option<u64>| v.map_or_else(|| "missing".into(), |v| format!("{:.4}", v as f64 / 1e6));
        for d in &self.deltas {
            let _ = writeln!(out, "{}\t{}\t{}\t{}{}", d.key, time(d.baseline_ns), time(d.current_ns),
                d.change.map_or_else(|| "n/a".into(), |v| format!("{:+.2}%", v * 100.0)), if d.regression { " REGRESSION" } else { "" });
        }
        for issue in &self.issues { let _ = writeln!(out, "{issue}"); }
        out
    }
}
