//! Rust allocator instrumentation, enabled only outside the latency samples.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct Allocator;
static TRACK: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards the allocator's pointer/layout contracts unchanged to System.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: the caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the original allocation's pointer/layout are forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        // SAFETY: forwards the caller's valid allocation and requested size unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

/// Counts allocations during a separate window, after all latency samples.
pub fn measure(run: impl FnOnce()) -> (usize, usize) {
    COUNT.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    run();
    TRACK.store(false, Ordering::Relaxed);
    (COUNT.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}
