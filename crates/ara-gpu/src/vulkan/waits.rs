//! Host time inside explicit GPU/presentation waits, independent of GPU timestamps.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Cumulative host wall time in leaf driver waits on one device. Snapshot on the
/// owning frame thread; differences exclude waits without subtracting GPU time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostWaitStats {
    /// Process-local logical-device generation; distinguishes counter resets.
    pub generation: u64,
    /// Time spent in timeline, upload-idle and presentation wait calls.
    pub elapsed: Duration,
    /// Number of those driver calls (including already-completed waits).
    pub calls: u64,
}
impl HostWaitStats {
    /// Host wait elapsed since the frame began. `None` as the prior snapshot
    /// means the device was created in this frame; replacing a device is unavailable.
    pub fn elapsed_since(self, previous: Option<Self>) -> Option<Duration> {
        if previous.is_some_and(|before| before.generation != self.generation) {
            return None;
        }
        self.elapsed
            .checked_sub(previous.map_or(Duration::ZERO, |before| before.elapsed))
    }
}

pub(crate) struct Counter {
    generation: u64,
    nanoseconds: AtomicU64,
    calls: AtomicU64,
}
impl Default for Counter {
    fn default() -> Self {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        Self {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            nanoseconds: AtomicU64::new(0),
            calls: AtomicU64::new(0),
        }
    }
}
impl Counter {
    pub fn snapshot(&self) -> HostWaitStats {
        HostWaitStats {
            generation: self.generation,
            elapsed: Duration::from_nanos(self.nanoseconds.load(Ordering::Relaxed)),
            calls: self.calls.load(Ordering::Relaxed),
        }
    }
    pub fn measure<T>(&self, operation: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = operation();
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.nanoseconds.fetch_add(elapsed, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wait_time_is_cumulative_and_driver_errors_are_preserved() {
        let counter = Counter::default();
        let result = counter.measure(|| {
            std::thread::sleep(Duration::from_millis(2));
            Err::<(), _>("driver error")
        });
        assert_eq!(result, Err("driver error"));
        let first = counter.snapshot();
        assert_eq!(first.calls, 1);
        assert!(first.elapsed >= Duration::from_millis(2));
        assert_eq!(counter.measure(|| 42), 42);
        let second = counter.snapshot();
        assert_eq!(second.calls, 2);
        assert!(second.elapsed >= first.elapsed);
        assert!(second.elapsed_since(Some(first)).is_some());
        assert_eq!(second.elapsed_since(None), Some(second.elapsed));
        assert!(
            Counter::default()
                .snapshot()
                .elapsed_since(Some(first))
                .is_none()
        );
    }
}
