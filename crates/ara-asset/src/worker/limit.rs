//! Bounded native admission: metadata priority and fair CPU/encoder lanes.
use std::{
    cell::Cell,
    sync::{Mutex, OnceLock},
};

/// Scheduling classes describe requests, never asset names or cache identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Work {
    Metadata,
    Cpu,
    Encoded,
}

#[derive(Debug, Default)]
struct State {
    active: [usize; 3],
    waiting: [usize; 3],
}

#[derive(Debug)]
struct Limit {
    capacity: usize,
    state: Mutex<State>,
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
        state: Mutex::default(),
    })
}

/// Held until confirmed child exit, independently of polling its terminal reply.
pub(super) struct Permit {
    limit: &'static Limit,
    work: Work,
}

/// One queued request. Metadata keeps a slot; CPU-only and encoded cooks
/// each keep one when both are waiting. Empty lanes reserve no process capacity.
pub(super) struct Admission {
    limit: &'static Limit,
    work: Work,
    queued: Cell<bool>,
}

impl Admission {
    pub(super) fn new(work: Work) -> Self {
        Self::at(global(), work)
    }

    fn at(limit: &'static Limit, work: Work) -> Self {
        let mut state = limit.state.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting[work as usize] += 1;
        Self {
            limit,
            work,
            queued: Cell::new(true),
        }
    }
    pub(super) fn try_acquire(&self) -> Option<Permit> {
        // This tiny transaction never holds the lock while spawning, doing I/O,
        // running an importer, or waiting for child exit.
        if !self.queued.get() {
            return None;
        }
        let mut state = self.limit.state.lock().unwrap_or_else(|e| e.into_inner());
        let index = self.work as usize;
        let active = state.active.iter().sum::<usize>();
        if active >= self.limit.capacity {
            return None;
        }
        if self.work != Work::Metadata {
            if state.waiting[Work::Metadata as usize] > 0 && active >= self.limit.capacity - 1 {
                return None;
            }
            let other = match self.work {
                Work::Cpu => Work::Encoded,
                Work::Encoded => Work::Cpu,
                Work::Metadata => unreachable!("metadata was excluded"),
            } as usize;
            if self.limit.capacity > 1
                && state.waiting[other] > 0
                && state.active[index] >= self.limit.capacity - 1
            {
                return None;
            }
        }
        state.waiting[index] -= 1;
        self.queued.set(false);
        state.active[index] += 1;
        Some(Permit {
            limit: self.limit,
            work: self.work,
        })
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.queued.get() {
            let mut state = self.limit.state.lock().unwrap_or_else(|e| e.into_inner());
            state.waiting[self.work as usize] -= 1;
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.limit.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active[self.work as usize] -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(capacity: usize) -> &'static Limit {
        Box::leak(Box::new(Limit {
            capacity,
            state: Mutex::default(),
        }))
    }

    #[test]
    fn metadata_precedes_bulk_and_cancelled_tickets_stop_reserving() {
        let limit = limit(1);
        let running = Admission::at(limit, Work::Cpu)
            .try_acquire()
            .expect("initial cook");
        let bulk = Admission::at(limit, Work::Cpu);
        let metadata = Admission::at(limit, Work::Metadata);
        assert!(metadata.try_acquire().is_none());
        drop(running);
        assert!(bulk.try_acquire().is_none());
        let running = metadata.try_acquire().expect("metadata priority");
        drop(metadata);
        assert!(bulk.try_acquire().is_none());
        drop(running);
        let cancelled = Admission::at(limit, Work::Metadata);
        assert!(bulk.try_acquire().is_none());
        drop(cancelled);
        assert!(bulk.try_acquire().is_some());
    }

    #[test]
    fn metadata_reserves_only_one_slot_and_keeps_the_same_cap() {
        let limit = limit(4);
        let metadata = Admission::at(limit, Work::Metadata);
        let cooks: Vec<_> = (0..3)
            .map(|_| {
                Admission::at(limit, Work::Cpu)
                    .try_acquire()
                    .expect("bulk slot")
            })
            .collect();
        let bulk = Admission::at(limit, Work::Cpu);
        assert!(bulk.try_acquire().is_none());
        let reserved = metadata.try_acquire().expect("metadata slot");
        assert!(bulk.try_acquire().is_none());
        drop(reserved);
        assert!(bulk.try_acquire().is_some());
        drop(cooks);
    }

    #[test]
    fn each_bulk_class_gets_a_slot_without_starving_the_other() {
        for (busy, waiting) in [(Work::Encoded, Work::Cpu), (Work::Cpu, Work::Encoded)] {
            let limit = limit(4);
            let mut running: Vec<_> = (0..4)
                .map(|_| {
                    Admission::at(limit, busy)
                        .try_acquire()
                        .expect("idle capacity")
                })
                .collect();
            let more = Admission::at(limit, busy);
            let other = Admission::at(limit, waiting);
            drop(running.pop());
            assert!(more.try_acquire().is_none(), "other lane keeps one slot");
            let reserved = other.try_acquire().expect("other class is admitted");
            assert!(more.try_acquire().is_none(), "total cap remains four");
            drop(reserved);
            assert!(more.try_acquire().is_some(), "empty lanes reserve nothing");
        }
    }

    #[test]
    fn an_unpolled_pending_job_does_not_block_later_jobs() {
        let limit = limit(1);
        let unpolled = Admission::at(limit, Work::Cpu);
        let later = Admission::at(limit, Work::Cpu);
        let running = later.try_acquire().expect("polled work progresses");
        assert!(
            later.try_acquire().is_none(),
            "a request launches only once"
        );
        assert!(unpolled.try_acquire().is_none(), "capacity remains bounded");
        drop(running);
        assert!(unpolled.try_acquire().is_some());
    }

    #[test]
    fn cancelling_the_other_lane_releases_its_reserved_capacity() {
        let limit = limit(4);
        let running: Vec<_> = (0..3)
            .map(|_| {
                Admission::at(limit, Work::Encoded)
                    .try_acquire()
                    .expect("slot")
            })
            .collect();
        let cancelled = Admission::at(limit, Work::Cpu);
        let encoded = Admission::at(limit, Work::Encoded);
        assert!(encoded.try_acquire().is_none());
        drop(cancelled);
        assert!(encoded.try_acquire().is_some());
        drop(running);
    }

    #[test]
    fn single_slot_never_reserves_a_parallel_lane() {
        let limit = limit(1);
        let cpu = Admission::at(limit, Work::Cpu);
        let encoded = Admission::at(limit, Work::Encoded);
        let running = encoded.try_acquire().expect("one polled request");
        assert!(cpu.try_acquire().is_none());
        drop(running);
        assert!(cpu.try_acquire().is_some());
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
