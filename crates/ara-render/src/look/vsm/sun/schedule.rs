//! Page redraw scheduling for the sun page service: two layers (the static sun pages
//! with their near ring, and the dynamic layer of movers, wind and skinned casters),
//! each under its own millisecond cap (council-v3 section 5: 1.0 ms on the 760M, 0.15
//! ms on the 4070 SUPER).
//!
//! A resident page becomes demand once its lag bound reaches `trigger x line` texels
//! and is due when it would reach the line (one texel of that page, half a texel in
//! the near ring). Demand is drawn earliest deadline first (cold pages, whose pixels
//! fall back to coarser levels, ahead of everything, coarser before finer), then by
//! error, then by key, so the order is deterministic. What the cap cannot draw stays
//! queued as debt with its age and lag: excess demand is never a quiet clamp, and the
//! counters say how far each layer is behind ([`Counters`]).
use serde::Serialize;

/// Which layer a page's demand belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum Layer {
    /// The sun's static casters (and the near ring), driven by sun motion.
    Static,
    /// Pages a moving, wind-driven or skinned caster touched since they were drawn.
    Dynamic,
}

/// One demanded page this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Demand<K> {
    pub key: K,
    /// Never drawn (or invalidated without a draw): its pixels use coarser levels.
    pub cold: bool,
    /// Clipmap level (coarser cold pages draw first: finer ones fall back to them).
    pub level: u8,
    /// The page's current lag bound in its texels (infinite: unknown, a mover with no
    /// displacement bound, or cold).
    pub lag: f32,
    /// How many texels the bound grows per frame at the current sun speed.
    pub rate: f32,
    /// The page's line (1 texel, or 0.5 in the near ring).
    pub line: f32,
    /// Whether the page is in the near ring.
    pub near: bool,
    /// Frame since which the page has been demand (debt age).
    pub since: u64,
}

impl<K> Demand<K> {
    /// Frames left before the page's lag reaches its line (negative: already past).
    pub(crate) fn deadline(&self) -> f32 {
        if !self.lag.is_finite() {
            return f32::NEG_INFINITY;
        }
        if self.rate <= 0. {
            return if self.lag >= self.line {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            };
        }
        (self.line - self.lag) / self.rate
    }
}

/// Orders demand: cold first (coarser before finer), then earliest deadline, then
/// largest error, then key.
pub(crate) fn order<K: Ord + Copy>(demand: &mut [Demand<K>]) {
    demand.sort_by(|a, b| {
        b.cold
            .cmp(&a.cold)
            .then_with(|| {
                if a.cold && b.cold {
                    b.level.cmp(&a.level)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| a.deadline().total_cmp(&b.deadline()))
            .then_with(|| b.lag.total_cmp(&a.lag))
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// A layer's runtime protection: a per-frame cap in GPU milliseconds, converted to a
/// page count through the measured cost of past draws: the median milliseconds per page
/// of the last 9 frames that drew pages (pass overhead included, so small packets are
/// sized conservatively; the median ignores clock-ramp outliers that a maximum would
/// keep for the whole window). At least one page draws each frame, so the queue drains.
#[derive(Debug, Clone)]
pub(crate) struct Budget {
    /// The cap; `None` draws every page of demand (the uncapped replay).
    pub cap_ms: Option<f32>,
    /// Recent (pages, ms) of frames that drew pages.
    window: [(f32, f32); 9],
    next: usize,
    /// Measurements so far (the seed sizes packets until the first).
    measured: usize,
}

/// Assumed cost of a page before any has been measured: conservative (a dense page).
const SEED_MS_PER_PAGE: f32 = 0.01;

impl Budget {
    pub(crate) fn new(cap_ms: Option<f32>) -> Self {
        Self {
            cap_ms,
            window: [(0., 0.); 9],
            next: 0,
            measured: 0,
        }
    }

    /// Learns from a completed frame that drew `pages` in `ms` of GPU time.
    pub(crate) fn measured(&mut self, pages: usize, ms: f64) {
        if pages == 0 || !ms.is_finite() || ms < 0. {
            return;
        }
        self.window[self.next] = (pages as f32, ms as f32);
        self.next = (self.next + 1) % self.window.len();
        self.measured += 1;
    }

    /// Pages this frame may draw.
    pub(crate) fn pages(&self) -> usize {
        let Some(cap) = self.cap_ms else {
            return usize::MAX;
        };
        let mut per: Vec<f32> = self.window[..self.measured.min(self.window.len())]
            .iter()
            .map(|s| s.1 / s.0)
            .collect();
        per.sort_by(f32::total_cmp);
        let per = per.get(per.len() / 2).copied().unwrap_or(SEED_MS_PER_PAGE);
        ((cap / per.max(1e-6)).floor().max(1.)) as usize
    }
}

/// One layer's counters, field for field T1's `StageCounters` (lab contract v1):
/// absent values serialize as null. Lags are page texels; an unbounded lag (a deferred
/// mover page without a displacement bound) reports [`UNBOUNDED`].
#[derive(Debug, Default, Clone, Serialize)]
pub struct Counters {
    /// GPU milliseconds of the layer's page draws (filled from completed pass timings).
    pub gpu_ms: Option<f64>,
    /// The layer's cap (null on the uncapped replay).
    pub cap_ms: Option<f64>,
    /// Pages demanded this frame (cold pages plus pages at or past their trigger).
    pub demand: Option<u64>,
    /// Demanded pages left queued after this frame's draws.
    pub debt: Option<u64>,
    /// The unit of `demand`/`debt` ("pages").
    pub work_unit: Option<String>,
    /// Age in frames of the oldest queued page.
    pub oldest_debt_frames: Option<u32>,
    /// Largest lag bound over demanded resident pages after the draws (page texels).
    pub edge_lag_page_texels: Option<f64>,
    /// The same over near-ring pages.
    pub near_edge_lag_page_texels: Option<f64>,
    /// Resident pages read this frame past their line.
    pub deadline_misses: Option<u64>,
    /// Atlas, page table and request storage of the service.
    pub resident_bytes: Option<u64>,
}

/// Lag reported for a deferred page whose motion has no bound.
pub const UNBOUNDED: f64 = 1e9;

/// Splits ordered `demand` into this frame's draws (at most `budget` pages) and the
/// queue left behind, and fills the layer's counters from what is left.
pub(crate) fn select<K: Copy>(
    demand: &[Demand<K>],
    budget: usize,
    frame: u64,
    counters: &mut Counters,
) -> (Vec<K>, Vec<K>) {
    let n = demand.len().min(budget);
    let (draw, rest) = demand.split_at(n);
    counters.demand = Some(demand.len() as u64);
    counters.debt = Some(rest.len() as u64);
    counters.work_unit = Some("pages".into());
    counters.oldest_debt_frames = Some(
        rest.iter()
            .map(|d| frame.saturating_sub(d.since) as u32)
            .max()
            .unwrap_or(0),
    );
    // Cold pages are not read (their pixels use coarser resident levels), so lag counts
    // only resident pages; a deferred page past its line is a deadline miss.
    let lag = |near: bool| {
        rest.iter()
            .filter(|d| !d.cold && (!near || d.near))
            .map(|d| {
                if d.lag.is_finite() {
                    f64::from(d.lag)
                } else {
                    UNBOUNDED
                }
            })
            .fold(0., f64::max)
    };
    counters.edge_lag_page_texels = Some(lag(false));
    counters.near_edge_lag_page_texels = Some(lag(true));
    counters.deadline_misses =
        Some(rest.iter().filter(|d| !d.cold && d.lag > d.line).count() as u64);
    (
        draw.iter().map(|d| d.key).collect(),
        rest.iter().map(|d| d.key).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demand(key: u32, lag: f32, rate: f32, cold: bool, level: u8) -> Demand<u32> {
        Demand {
            key,
            cold,
            level,
            lag,
            rate,
            line: 1.,
            near: level < 2,
            since: 10,
        }
    }

    #[test]
    fn cold_pages_draw_first_then_earliest_deadline() {
        let mut d = vec![
            demand(1, 0.6, 0.01, false, 3),
            demand(2, 0.5, 0.2, false, 3),
            demand(3, f32::INFINITY, 0., true, 1),
            demand(4, f32::INFINITY, 0., true, 5),
            demand(5, 1.4, 0.1, false, 0),
        ];
        order(&mut d);
        let keys: Vec<u32> = d.iter().map(|d| d.key).collect();
        // Cold coarse, cold fine, past its line, due in 2.5 frames, due in 40 frames.
        assert_eq!(keys, [4, 3, 5, 2, 1]);
    }

    #[test]
    fn excess_demand_queues_with_its_lag_and_age() {
        let mut d = vec![
            demand(1, 0.9, 0.2, false, 0),
            demand(2, 1.2, 0.2, false, 4),
            demand(3, 0.7, 0.2, false, 4),
        ];
        order(&mut d);
        let mut c = Counters::default();
        let (draw, rest) = select(&d, 1, 15, &mut c);
        assert_eq!(draw, [2]);
        assert_eq!(rest, [1, 3]);
        assert_eq!(c.demand, Some(3));
        assert_eq!(c.debt, Some(2));
        assert_eq!(c.oldest_debt_frames, Some(5));
        assert_eq!(c.edge_lag_page_texels, Some(f64::from(0.9_f32)));
        assert_eq!(c.near_edge_lag_page_texels, Some(f64::from(0.9_f32)));
        assert_eq!(c.deadline_misses, Some(0));
        // Unbounded deferred movers report as unbounded, never as zero.
        let movers = [demand(9, f32::INFINITY, 0., false, 2)];
        let (_, rest) = select(&movers, 0, 15, &mut c);
        assert_eq!(rest, [9]);
        assert_eq!(c.edge_lag_page_texels, Some(UNBOUNDED));
        assert_eq!(c.deadline_misses, Some(1));
    }

    #[test]
    fn budgets_follow_the_largest_measured_page_cost() {
        let mut b = Budget::new(Some(0.15));
        assert_eq!(b.pages(), 15);
        // A slow warm-up frame (idle clocks), then steady 0.001 ms per page plus 0.02.
        b.measured(15, 0.5);
        assert_eq!(b.pages(), 4);
        for _ in 0..4 {
            b.measured(100, 0.12);
        }
        let steady = b.pages();
        assert!((124..=125).contains(&steady), "{steady}");
        // One outlier among nine does not move the median.
        b.measured(100, 0.9);
        assert_eq!(b.pages(), steady);
        assert_eq!(Budget::new(None).pages(), usize::MAX);
    }
}
