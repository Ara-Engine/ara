//! Process-wide admission for native importers, including dependency discovery.
use std::sync::{
    OnceLock,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug)]
struct Limit {
    capacity: usize,
    active: AtomicUsize,
    discovery_waiters: AtomicUsize,
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
        discovery_waiters: AtomicUsize::new(0),
    })
}

/// Held until the admitted child has exited and been reaped, including cancellation.
pub(super) struct Permit(&'static Limit);

/// Short discovery/version requests reserve a slot ahead of queued encoders. They
/// always synchronously polled by ProcessWorker; cancellation drops their ticket.
pub(super) struct Admission {
    limit: &'static Limit,
    discovery: bool,
}

impl Admission {
    pub(super) fn new(discovery: bool) -> Self {
        Self::at(global(), discovery)
    }
    fn at(limit: &'static Limit, discovery: bool) -> Self {
        if discovery {
            limit.discovery_waiters.fetch_add(1, Ordering::AcqRel);
        }
        Self { limit, discovery }
    }
    pub(super) fn try_acquire(&self) -> Option<Permit> {
        let reserve = !self.discovery && self.limit.discovery_waiters.load(Ordering::Acquire) > 0;
        Permit::acquire(self.limit, usize::from(reserve))
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.discovery {
            self.limit.discovery_waiters.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Permit {
    fn acquire(limit: &'static Limit, reserved: usize) -> Option<Self> {
        limit
            .active
            .try_update(Ordering::Acquire, Ordering::Relaxed, |active| {
                (active < limit.capacity.saturating_sub(reserved)).then_some(active + 1)
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
    use super::*;

    #[test]
    fn discovery_precedes_queued_cooks_and_cancellation_removes_priority() {
        let limit = Box::leak(Box::new(Limit {
            capacity: 1,
            active: AtomicUsize::new(0),
            discovery_waiters: AtomicUsize::new(0),
        }));
        let bulk = Admission::at(limit, false);
        let running = bulk.try_acquire().expect("first cook");
        let discovery = Admission::at(limit, true);
        assert!(
            discovery.try_acquire().is_none(),
            "capacity remains bounded"
        );
        drop(running);
        assert!(
            bulk.try_acquire().is_none(),
            "queued cook cannot starve discovery"
        );
        let running = discovery.try_acquire().expect("metadata request");
        drop(discovery);
        assert!(
            bulk.try_acquire().is_none(),
            "discovery still occupies its slot"
        );
        drop(running);
        let cancelled = Admission::at(limit, true);
        assert!(bulk.try_acquire().is_none());
        drop(cancelled);
        assert!(
            bulk.try_acquire().is_some(),
            "cancelled metadata does not block cooks"
        );
    }

    #[test]
    fn queued_discovery_keeps_one_slot_without_stalling_other_cooks() {
        let limit = Box::leak(Box::new(Limit {
            capacity: 4,
            active: AtomicUsize::new(0),
            discovery_waiters: AtomicUsize::new(0),
        }));
        let discovery = Admission::at(limit, true);
        let bulk = Admission::at(limit, false);
        let cooks: Vec<_> = (0..3)
            .map(|_| bulk.try_acquire().expect("unreserved slot"))
            .collect();
        assert!(bulk.try_acquire().is_none());
        let metadata = discovery.try_acquire().expect("reserved slot");
        assert!(
            discovery.try_acquire().is_none(),
            "shared capacity still applies"
        );
        drop(metadata);
        drop(discovery);
        assert!(bulk.try_acquire().is_some());
        drop(cooks);
    }

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
