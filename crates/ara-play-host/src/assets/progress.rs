use std::time::Duration;

/// Completed requested cooks while an export waits for immutable generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookProgress {
    /// Requested assets with a published generation and no outstanding cook.
    pub completed: usize,
    /// Number of requested imported assets.
    pub total: usize,
    /// Total elapsed cooking time, including queued jobs.
    pub elapsed: Duration,
    /// Time since another requested cook completed.
    pub stalled: Duration,
}

pub(super) struct CookWatch {
    completed: usize,
    last_completion: Duration,
}
impl CookWatch {
    pub(super) fn new() -> Self {
        Self {
            completed: 0,
            last_completion: Duration::ZERO,
        }
    }
    pub(super) fn observe(
        &mut self,
        completed: usize,
        total: usize,
        elapsed: Duration,
    ) -> CookProgress {
        if completed > self.completed {
            self.completed = completed;
            self.last_completion = elapsed;
        }
        CookProgress {
            completed,
            total,
            elapsed,
            stalled: elapsed.saturating_sub(self.last_completion),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_extend_a_cold_queue_beyond_the_stall_limit() {
        let mut watch = CookWatch::new();
        let limit = Duration::from_secs(10);
        for completed in 0..100 {
            let elapsed = Duration::from_secs(completed as u64 * 9);
            assert!(watch.observe(completed, 100, elapsed).stalled < limit);
            assert!(
                watch
                    .observe(completed, 100, elapsed + Duration::from_secs(8))
                    .stalled
                    < limit
            );
        }
        assert_eq!(
            watch.observe(100, 100, Duration::from_secs(900)).stalled,
            Duration::ZERO
        );
        assert!(watch.observe(100, 101, Duration::from_secs(910)).stalled >= limit);
    }
}
