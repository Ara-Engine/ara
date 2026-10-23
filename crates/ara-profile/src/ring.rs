use crate::allocator::local_bytes;
use std::{
    cell::{RefCell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

pub(crate) struct Control {
    pub id: u64,
    #[cfg(not(feature = "tracy"))]
    pub names: std::sync::Mutex<Names>,
    pub enabled: std::sync::atomic::AtomicBool,
    pub frame: AtomicU64,
    pub clock: crate::clock::Clock,
}
#[derive(Default)]
#[cfg(not(feature = "tracy"))]
pub(crate) struct Names {
    pub values: Vec<String>,
    pub ids: std::collections::BTreeMap<String, u32>,
}
impl Control {
    pub fn now(&self) -> u64 {
        self.clock.now()
    }
}
#[derive(Clone, Copy)]
pub(crate) enum Name {
    Static(&'static str),
    #[cfg(not(feature = "tracy"))]
    Callsite(&'static crate::Callsite),
    #[cfg(not(feature = "tracy"))]
    Registered(u32),
}
#[derive(Clone, Copy)]
pub(crate) struct RawSpan {
    pub frame: u64,
    pub name: Name,
    pub id: u64,
    pub parent: u64,
    pub start: u64,
    pub duration: u64,
    pub self_ns: u64,
    pub allocated: u64,
    pub self_allocated: u64,
}
pub(crate) struct Ring {
    slots: Box<[UnsafeCell<MaybeUninit<RawSpan>>]>,
    read: AtomicUsize,
    write: AtomicUsize,
    pub dropped: AtomicU64,
}
// SAFETY: one TLS producer writes a slot before publishing `write` (release).
// The sole collector reads only published slots (acquire), then releases `read`.
// No slot is accessed by both endpoints concurrently, including on wraparound.
unsafe impl Sync for Ring {}
impl Ring {
    pub fn new(capacity: usize) -> Self {
        debug_assert!(
            capacity.is_power_of_two(),
            "Profiler validates ring capacity"
        );
        Self {
            slots: (0..capacity)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }
    pub fn pop(&self) -> Option<RawSpan> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: acquire observed an initialized slot; producer cannot reuse it yet.
        let span = unsafe { (*self.slots[read & (self.slots.len() - 1)].get()).assume_init_read() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(span)
    }
}

struct Active {
    frame: u64,
    name: Name,
    id: u64,
    parent: u64,
    tree_parent: u64,
    start: u64,
    children: u64,
    allocated: u64,
    child_allocated: u64,
}
pub(crate) struct Producer {
    pub control: Arc<Control>,
    pub ring: Arc<Ring>,
    stack: [Option<Active>; 128],
    depth: usize,
    next: u64,
    write: usize,
    published: usize,
    cached_read: usize,
    dropped: u64,
    pub frame_override: Option<u64>,
}
impl Producer {
    pub fn new(control: Arc<Control>, ring: Arc<Ring>) -> Self {
        let write = ring.write.load(Ordering::Relaxed);
        let cached_read = ring.read.load(Ordering::Acquire);
        Self {
            control,
            ring,
            stack: std::array::from_fn(|_| None),
            depth: 0,
            next: 0,
            write,
            published: write,
            cached_read,
            dropped: 0,
            frame_override: None,
        }
    }
    #[inline(always)]
    fn push(&mut self, span: RawSpan) {
        if self.write.wrapping_sub(self.cached_read) >= self.ring.slots.len() {
            self.dropped += 1;
            return;
        }
        // SAFETY: cached_read was acquired at the previous publication boundary.
        // This sole producer owns every slot up to capacity beyond that cursor.
        // No consumer can see this new slot until publish's release store.
        unsafe {
            (*self.ring.slots[self.write & (self.ring.slots.len() - 1)].get()).write(span);
        }
        self.write = self.write.wrapping_add(1);
    }
    /// Batch handoff only at frame/job boundaries, never at a normal scope edge.
    pub fn publish(&mut self) {
        if self.write != self.published {
            self.ring.write.store(self.write, Ordering::Release);
            self.published = self.write;
        }
        if self.dropped != 0 {
            self.ring.dropped.fetch_add(self.dropped, Ordering::Relaxed);
            self.dropped = 0;
        }
        self.cached_read = self.ring.read.load(Ordering::Acquire);
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        self.publish();
    }
}
thread_local! { pub(crate) static PRODUCER: RefCell<Option<Producer>> = const { RefCell::new(None) }; }

pub(crate) fn mark_dropped() {
    PRODUCER.with_borrow_mut(|p| {
        if let Some(p) = p {
            p.dropped += 1;
        }
    });
}

#[inline]
pub(crate) fn begin(name: Name, display: &str) -> Option<(u64, u64)> {
    let _ = display;
    PRODUCER.with_borrow_mut(|slot| {
        let p = slot.as_mut()?;
        if !p.control.enabled.load(Ordering::Relaxed) {
            return None;
        }
        let frame = p
            .frame_override
            .unwrap_or_else(|| p.control.frame.load(Ordering::Acquire));
        if frame == 0 {
            return None;
        }
        if p.depth == p.stack.len() {
            p.dropped += 1;
            return None;
        }
        p.next = p.next.checked_add(1)?;
        let parent = p
            .depth
            .checked_sub(1)
            .and_then(|i| p.stack[i].as_ref())
            .map_or(0, |s| s.id);
        let tree_parent = p
            .depth
            .checked_sub(1)
            .and_then(|i| p.stack[i].as_ref())
            .filter(|s| s.frame == frame)
            .map_or(0, |s| s.id);
        p.stack[p.depth] = Some(Active {
            frame,
            name,
            id: p.next,
            parent,
            tree_parent,
            start: p.control.clock.read(),
            children: 0,
            allocated: local_bytes(),
            child_allocated: 0,
        });
        p.depth += 1;
        Some((p.control.id, p.next))
    })
}
#[inline]
pub(crate) fn end(owner: u64, id: u64) {
    let _ = PRODUCER.try_with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return;
        };
        let Some(p) = slot.as_mut() else {
            return;
        };
        if p.control.id != owner {
            return;
        }
        let Some(top) = p.depth.checked_sub(1) else {
            return;
        };
        let i = if p.stack[top].as_ref().is_some_and(|s| s.id == id) {
            top
        } else if let Some(i) = p.stack[..p.depth]
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.id == id))
        {
            i
        } else {
            return;
        };
        // Explicitly dropped guards may exit out of order. Their children keep
        // their original parent, but the malformed nesting is marked incomplete.
        if i + 1 != p.depth {
            p.dropped += 1;
        }
        let Some(active) = p.stack[i].take() else {
            return;
        };
        while p.depth > 0 && p.stack[p.depth - 1].is_none() {
            p.depth -= 1;
        }
        let after = p.control.clock.read();
        if after < active.start {
            p.dropped += 1;
        }
        let duration = after.saturating_sub(active.start);
        let allocated = local_bytes().wrapping_sub(active.allocated);
        if active.parent != 0 {
            let parent = if p.depth > 0
                && p.stack[p.depth - 1]
                    .as_ref()
                    .is_some_and(|s| s.id == active.parent)
            {
                p.stack[p.depth - 1].as_mut()
            } else {
                p.stack[..p.depth]
                    .iter_mut()
                    .flatten()
                    .find(|s| s.id == active.parent)
            };
            if let Some(parent) = parent {
                parent.children = parent.children.saturating_add(duration);
                parent.child_allocated = parent.child_allocated.saturating_add(allocated);
            }
        }
        p.push(RawSpan {
            frame: active.frame,
            name: active.name,
            id,
            parent: active.tree_parent,
            start: active.start,
            duration,
            self_ns: duration.saturating_sub(active.children),
            allocated,
            self_allocated: allocated.saturating_sub(active.child_allocated),
        });
    });
}

/// Thread-bound RAII scope; dropping completes the CPU span on its entering thread.
#[must_use]
pub struct SpanGuard {
    id: u64,
    owner: u64,
    _thread: PhantomData<Rc<()>>,
}
impl SpanGuard {
    #[cfg(not(feature = "tracy"))]
    #[inline]
    pub(crate) fn at(site: &'static crate::Callsite) -> Self {
        let (owner, id) = begin(Name::Callsite(site), site.name()).unwrap_or((0, 0));
        Self {
            id,
            owner,
            _thread: PhantomData,
        }
    }
    #[inline(always)]
    pub(crate) const fn inactive() -> Self {
        Self {
            id: 0,
            owner: 0,
            _thread: PhantomData,
        }
    }
    #[inline]
    pub(crate) fn new(name: &'static str) -> Self {
        let (owner, id) = begin(Name::Static(name), name).unwrap_or((0, 0));
        Self {
            id,
            owner,
            _thread: PhantomData,
        }
    }
}
impl Drop for SpanGuard {
    #[inline]
    fn drop(&mut self) {
        if self.id != 0 {
            end(self.owner, self.id);
        }
    }
}
