use std::{cell::{RefCell, UnsafeCell}, marker::PhantomData, mem::MaybeUninit,
    rc::Rc, sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}}, time::Instant};
use crate::allocator::local_bytes;

pub(crate) struct Control {
    pub id: u64,
    pub names: std::sync::Mutex<Vec<String>>,
    pub enabled: std::sync::atomic::AtomicBool,
    pub frame: AtomicU64,
    pub origin: Instant,
}
impl Control {
    pub fn now(&self) -> u64 { self.origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64 }
}
#[derive(Clone, Copy)]
pub(crate) enum Name { Static(&'static str), Registered(u32) }
#[derive(Clone, Copy)]
pub(crate) struct RawSpan {
    pub frame: u64, pub name: Name, pub id: u64, pub parent: u64,
    pub start: u64, pub duration: u64, pub self_ns: u64,
    pub allocated: u64, pub self_allocated: u64,
}
pub(crate) struct Ring {
    slots: Box<[UnsafeCell<MaybeUninit<RawSpan>>]>,
    read: AtomicUsize, write: AtomicUsize,
    pub dropped: AtomicU64,
}
// SAFETY: one TLS producer writes a slot before publishing `write` (release).
// The sole collector reads only published slots (acquire), then releases `read`.
// No slot is accessed by both endpoints concurrently, including on wraparound.
unsafe impl Sync for Ring {}
impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self { slots: (0..capacity).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect(),
            read: AtomicUsize::new(0), write: AtomicUsize::new(0), dropped: AtomicU64::new(0) }
    }
    fn push(&self, span: RawSpan) {
        let write = self.write.load(Ordering::Relaxed);
        if write.wrapping_sub(self.read.load(Ordering::Acquire)) >= self.slots.len() {
            self.dropped.fetch_add(1, Ordering::Relaxed); return;
        }
        // SAFETY: this is the sole producer's unpublished slot, not held by the reader.
        unsafe { (*self.slots[write % self.slots.len()].get()).write(span); }
        self.write.store(write.wrapping_add(1), Ordering::Release);
    }
    pub fn pop(&self) -> Option<RawSpan> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) { return None; }
        // SAFETY: acquire observed an initialized slot; producer cannot reuse it yet.
        let span = unsafe { (*self.slots[read % self.slots.len()].get()).assume_init_read() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(span)
    }
}

struct Active {
    frame: u64, name: Name, id: u64, parent: u64, start: u64,
    children: u64, allocated: u64, child_allocated: u64,
    #[cfg(feature = "tracy")]
    _tracy: Option<tracy_client::Span>,
}
pub(crate) struct Producer {
    pub control: Arc<Control>, pub ring: Arc<Ring>,
    stack: [Option<Active>; 128], depth: usize, next: u64,
    pub frame_override: Option<u64>,
}
impl Producer {
    pub fn new(control: Arc<Control>, ring: Arc<Ring>) -> Self {
        Self { control, ring, stack: std::array::from_fn(|_| None), depth: 0, next: 0, frame_override: None }
    }
}
thread_local! { pub(crate) static PRODUCER: RefCell<Option<Producer>> = const { RefCell::new(None) }; }

pub(crate) fn begin(name: Name) -> Option<u64> {
    PRODUCER.with_borrow_mut(|slot| {
        let p = slot.as_mut()?;
        if !p.control.enabled.load(Ordering::Relaxed) { return None; }
        let frame = p.frame_override.unwrap_or_else(|| p.control.frame.load(Ordering::Acquire));
        if frame == 0 { return None; }
        if p.depth == p.stack.len() { p.ring.dropped.fetch_add(1, Ordering::Relaxed); return None; }
        p.next = p.next.checked_add(1)?;
        let parent = p.depth.checked_sub(1).and_then(|i| p.stack[i].as_ref()).map_or(0, |s| s.id);
        p.stack[p.depth] = Some(Active { frame, name, id: p.next, parent, start: p.control.now(), children: 0,
            allocated: local_bytes(), child_allocated: 0,
            #[cfg(feature = "tracy")]
            _tracy: tracy_client::Client::running().map(|c| c.span_alloc(Some(match name { Name::Static(name) => name, Name::Registered(_) => "tracing scope" }), "Ara scope", file!(), line!(), 0)),
        });
        p.depth += 1;
        Some(p.next)
    })
}
pub(crate) fn end(id: u64) {
    let _ = PRODUCER.try_with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else { return; };
        let Some(p) = slot.as_mut() else { return; };
        let Some(i) = p.stack[..p.depth].iter().position(|s| s.as_ref().is_some_and(|s| s.id == id)) else { return; };
        // Explicitly dropped guards may exit out of order. Their children keep
        // their original parent, but the malformed nesting is marked incomplete.
        if i + 1 != p.depth { p.ring.dropped.fetch_add(1, Ordering::Relaxed); }
        let Some(active) = p.stack[i].take() else { return; };
        while p.depth > 0 && p.stack[p.depth - 1].is_none() { p.depth -= 1; }
        let duration = p.control.now().saturating_sub(active.start);
        let allocated = local_bytes().wrapping_sub(active.allocated);
        if let Some(parent) = p.stack[..p.depth].iter_mut().flatten().find(|s| s.id == active.parent) {
            parent.children = parent.children.saturating_add(duration);
            parent.child_allocated = parent.child_allocated.saturating_add(allocated);
        }
        p.ring.push(RawSpan { frame: active.frame, name: active.name, id, parent: active.parent,
            start: active.start, duration, self_ns: duration.saturating_sub(active.children), allocated,
            self_allocated: allocated.saturating_sub(active.child_allocated) });
    });
}

/// Thread-bound RAII scope; dropping completes the CPU span on its entering thread.
#[must_use]
pub struct SpanGuard { id: Option<u64>, owner: u64, _thread: PhantomData<Rc<()>> }
impl SpanGuard {
    pub(crate) fn new(name: &'static str) -> Self {
        let id = begin(Name::Static(name));
        let owner = if id.is_some() { PRODUCER.with_borrow(|p| p.as_ref().map_or(0, |p| p.control.id)) } else { 0 };
        Self { id, owner, _thread: PhantomData }
    }
}
impl Drop for SpanGuard { fn drop(&mut self) {
    if let Some(id) = self.id {
        if PRODUCER.try_with(|p| p.borrow().as_ref().is_some_and(|p| p.control.id == self.owner)).unwrap_or(false) { end(id); }
    }
} }
