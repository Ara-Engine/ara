//! The bake's small allocation descriptor, published before tracing its probes.
use anyhow::{Result, ensure};

/// Dimensions of a probe volume, without any surface payload. A startup owner
/// can reserve and clear its GPU resources while acquisition continues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeLayout {
    /// First probe centre in world metres.
    pub origin: [f32; 3],
    /// Positive probe separation per axis, in metres.
    pub spacing: [f32; 3],
    /// Lattice dimensions (2..=256 per axis, at most 262,144 probes total).
    pub counts: [u32; 3],
}

impl ProbeLayout {
    /// Probe capacity; invalid oversized input saturates instead of overflowing.
    pub fn probe_count(&self) -> usize {
        self.counts
            .iter()
            .fold(1usize, |n, &c| n.saturating_mul(c as usize))
    }

    /// Check the same lattice limits as acquired probe surfaces before allocation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.counts.iter().all(|&c| (2..=256).contains(&c)) && self.probe_count() <= 1 << 18,
            "Invalid probe lattice"
        );
        ensure!(
            self.origin
                .iter()
                .chain(&self.spacing)
                .all(|v| v.is_finite())
                && self.spacing.iter().all(|&s| s > 0.),
            "Invalid probe positions"
        );
        Ok(())
    }

    pub(crate) fn bias(&self) -> f32 {
        (self.spacing.iter().copied().fold(f32::INFINITY, f32::min) * 0.2).min(0.15)
    }
}
