//! Tests share one process-wide diagnostics bus, and a drain gives each event to whichever
//! host drains it first ("one consumer per process is the intended setup"). Two editors alive
//! at once therefore steal each other's events: a test waiting for its own import failure
//! never sees it when another test's host drained it. Under test an editor holds the turn from
//! open to drop, so only one editor is alive per process at a time (re-entrant per thread).

use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::ThreadId;

/// The thread holding the turn and how many editors it has open.
static HELD: Mutex<Option<(ThreadId, usize)>> = Mutex::new(None);
static FREE: Condvar = Condvar::new();

/// One open editor's claim on the turn; releasing it hands the turn to the next waiter.
pub(crate) struct Turn;

impl Turn {
    /// Waits until no other thread holds the turn, then holds it.
    pub(crate) fn take() -> Self {
        let me = std::thread::current().id();
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match &mut *held {
                None => {
                    *held = Some((me, 1));
                    return Self;
                }
                Some((owner, open)) if *owner == me => {
                    *open += 1;
                    return Self;
                }
                Some(_) => held = FREE.wait(held).unwrap_or_else(PoisonError::into_inner),
            }
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, open)) = &mut *held
            && *open > 1
        {
            *open -= 1;
            return;
        }
        *held = None;
        FREE.notify_one();
    }
}
