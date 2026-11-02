//! Bench-only allocation high-water measurement. Input scene allocations precede
//! the measured scope; construction/advance temporaries and old publications count.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Measured;
#[global_allocator]
static ALLOCATOR: Measured = Measured;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}
// SAFETY: All allocation/alignment contracts are delegated unchanged to System.
// Counters neither dereference nor alter returned pointers.
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: caller's valid allocation layout is passed to System unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: caller guarantees pointer/layout belong to this allocator.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: caller guarantees a live allocation and valid requested size.
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            if size >= layout.size() {
                add(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Relaxed);
            }
        }
        result
    }
}

pub fn begin() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}
pub fn end(before: usize) -> (isize, usize) {
    (
        LIVE.load(Relaxed) as isize - before as isize,
        PEAK.load(Relaxed).saturating_sub(before),
    )
}
