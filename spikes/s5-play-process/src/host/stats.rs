//! Latency statistics for the report. Percentiles use the nearest-rank method.

use std::time::Duration;

use serde::Serialize;

/// Summary of a sample set, in milliseconds. `samples` keeps the raw values for re-analysis.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub n: usize,
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
    pub samples: Vec<f64>,
}

impl Stats {
    pub fn of(samples: &[f64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank = |p: f64| {
            let idx = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len().max(1)) - 1;
            sorted.get(idx).copied().unwrap_or(f64::NAN)
        };
        Self {
            n: sorted.len(),
            min: sorted.first().copied().unwrap_or(f64::NAN),
            p50: rank(0.50),
            p95: rank(0.95),
            max: sorted.last().copied().unwrap_or(f64::NAN),
            mean: sorted.iter().sum::<f64>() / sorted.len().max(1) as f64,
            samples: samples
                .iter()
                .map(|v| (v * 1000.0).round() / 1000.0)
                .collect(),
        }
    }

    /// `p50 / p95` for console tables.
    pub fn short(&self) -> String {
        format!("{:.2} / {:.2} ms", self.p50, self.p95)
    }
}

/// Duration in milliseconds.
pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Microseconds reported by a player, in milliseconds.
pub fn us_ms(us: u64) -> f64 {
    us as f64 / 1000.0
}
