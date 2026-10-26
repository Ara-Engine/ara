//! Counter churn is tolerated; only five seconds without a valid sample is a gap.
use super::model::Interval;
use serde::{Deserialize, Serialize};

pub(super) const LOSS_TIMEOUT_MS: u64 = 5000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Gap {
    #[serde(flatten)]
    pub interval: Interval,
    pub reason: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Retry {
    pub unix_ms: u64,
    pub reason: String,
}

/// UTC coverage includes failed polls. Gaps are separate from foreign GPU load.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Monitoring {
    pub start_unix_ms: Option<u64>,
    pub end_unix_ms: Option<u64>,
    pub retries: Vec<Retry>,
    pub gaps: Vec<Gap>,
}

impl Monitoring {
    fn advance(&mut self, at: u64, last_valid: Option<u64>, reason: &str) {
        let first = *self.start_unix_ms.get_or_insert(at.saturating_sub(1000));
        self.end_unix_ms = Some(self.end_unix_ms.unwrap_or(at).max(at));
        let start = last_valid.unwrap_or(first);
        if at.saturating_sub(start) < LOSS_TIMEOUT_MS {
            return;
        }
        if let Some(gap) = self.gaps.last_mut()
            && gap.interval.start_unix_ms == start
        {
            gap.interval.end_unix_ms = gap.interval.end_unix_ms.max(at);
        } else {
            self.gaps.push(Gap {
                interval: Interval {
                    start_unix_ms: start,
                    end_unix_ms: at,
                },
                reason: reason.into(),
            });
        }
    }

    pub fn valid(&mut self, at: u64, last_valid: Option<u64>) {
        let reason = self
            .retries
            .last()
            .map_or("no valid GPU sample for five seconds", |retry| {
                retry.reason.as_str()
            })
            .to_owned();
        self.advance(at, last_valid, &reason);
    }

    pub fn unavailable(&mut self, at: u64, last_valid: Option<u64>, reason: String) {
        self.advance(at, last_valid, &reason);
        self.retries.push(Retry {
            unix_ms: at,
            reason,
        });
    }

    /// Extend a stalled sampler through the actual end of the reading.
    pub fn finish(&mut self, at: u64, last_valid: Option<u64>) {
        self.valid(at, last_valid);
    }

    pub fn overlaps(&self, interval: Interval) -> bool {
        self.gaps.iter().any(|gap| gap.interval.overlaps(interval))
    }
}
