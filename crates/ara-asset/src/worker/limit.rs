//! Process-wide admission for native importers, including dependency discovery.
use std::sync::{
    OnceLock,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug)]
struct Limit {
    capacity: usize,
    active: AtomicUsize,
}

fn capacity(configured: Option<&str>, parallelism: usize) -> usize {
    configured
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(4)
        .min(parallelism.max(1))
}

fn global() -> &'static Limit {
    static LIMIT: OnceLock<Limit> = OnceLock::new();
    LIMIT.get_or_init(|| Limit {
        capacity: capacity(
            std::env::var("ARA_IMPORT_WORKERS").ok().as_deref(),
            std::thread::available_parallelism().map_or(1, usize::from),
        ),
        active: AtomicUsize::new(0),
    })
}

/// Held until the admitted child has exited and been reaped, including cancellation.
pub(super) struct Permit(&'static Limit);

impl Permit {
    pub(super) fn try_acquire() -> Option<Self> {
        let limit = global();
        limit
            .active
            .try_update(Ordering::Acquire, Ordering::Relaxed, |active| {
                (active < limit.capacity).then_some(active + 1)
            })
            .ok()
            .map(|_| Self(limit))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::capacity;

    #[test]
    fn configuration_defaults_and_never_exceeds_parallelism() {
        for available in [1, 2, 8, 64] {
            assert_eq!(capacity(None, available), 4.min(available));
            assert_eq!(capacity(Some("0"), available), 4.min(available));
            assert_eq!(capacity(Some("invalid"), available), 4.min(available));
            assert_eq!(capacity(Some("1000"), available), available);
            assert_eq!(capacity(Some("1"), available), 1);
        }
    }
}
