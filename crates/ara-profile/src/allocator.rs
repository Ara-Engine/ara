use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static LIVE: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
thread_local! { static LOCAL: Cell<u64> = const { Cell::new(0) }; }

/// Cumulative allocation request sample (not retained memory).
#[derive(Debug, Default, Clone, Copy)]
pub struct AllocationSample {
    /// Cumulative bytes requested, including successful reallocations.
    pub bytes: u64,
    /// Cumulative successful allocation/reallocation calls.
    pub calls: u64,
}
impl AllocationSample {
    /// Cumulative requested bytes on this thread, for allocation-free hot-loop checks.
    pub fn thread_bytes() -> u64 { local_bytes() }
    /// Process-wide snapshot; enabled only with the `allocation` feature.
    pub fn now() -> Self { Self { bytes: BYTES.load(Relaxed), calls: CALLS.load(Relaxed) } }
}

pub(crate) fn local_bytes() -> u64 { LOCAL.try_with(Cell::get).unwrap_or(0) }

/// Live requested heap bytes. Install `CountingAllocator<System>` in the dev executable.
pub fn heap_bytes() -> u64 { LIVE.load(Relaxed) }

/// Opt-in global allocator wrapper. Without `allocation`, delegates without counting.
/// The executable chooses this allocator; libraries never install one implicitly.
pub struct CountingAllocator<A>(pub A);

fn allocated(bytes: usize) {
    if cfg!(feature = "allocation") {
        LIVE.fetch_add(bytes as u64, Relaxed);
        BYTES.fetch_add(bytes as u64, Relaxed);
        CALLS.fetch_add(1, Relaxed);
        let _ = LOCAL.try_with(|v| v.set(v.get().wrapping_add(bytes as u64)));
    }
}
fn freed(bytes: usize) {
    if cfg!(feature = "allocation") { LIVE.fetch_sub(bytes as u64, Relaxed); }
}

// SAFETY: every operation delegates with the caller's original layout/pointer;
// accounting uses atomics and const TLS only and never allocates or unwinds.
unsafe impl<A: GlobalAlloc> GlobalAlloc for CountingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding GlobalAlloc's caller contract to the inner allocator.
        let p = unsafe { self.0.alloc(layout) };
        if !p.is_null() { allocated(layout.size()); }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding GlobalAlloc's caller contract unchanged.
        let p = unsafe { self.0.alloc_zeroed(layout) };
        if !p.is_null() { allocated(layout.size()); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        freed(layout.size());
        // SAFETY: pointer and layout are exactly those supplied by the caller.
        unsafe { self.0.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: forwarding the caller's valid allocation and new size unchanged.
        let next = unsafe { self.0.realloc(p, layout, size) };
        if !next.is_null() { freed(layout.size()); allocated(size); }
        next
    }
}
