//! A bounded single-producer single-consumer ring (Lamport queue).
//!
//! Each emitting thread owns the producer side of one ring; the bus consumer (the
//! host's drain) owns the consumer side of all of them. Pushing is two atomic loads,
//! a slot write and one release store: no locks, no allocation, no contention
//! between emitting threads.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Keeps the producer and consumer indices on separate cache lines.
#[repr(align(64))]
struct Padded(AtomicUsize);

/// The ring. `T` moves from the producer thread to the consumer thread.
pub(crate) struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Next slot the consumer reads (only the consumer stores it).
    head: Padded,
    /// Next slot the producer writes (only the producer stores it).
    tail: Padded,
}

// SAFETY: a slot is written only by the producer while it is outside
// `head..tail`, and read only by the consumer while it is inside; the release
// store of `tail` (resp. `head`) publishes the write (resp. the read) to the other
// side, so no slot is ever accessed by both threads at once. `T: Send` lets the
// values cross threads.
unsafe impl<T: Send> Sync for Ring<T> {}
// SAFETY: as above; the ring owns its values, which are `Send`.
unsafe impl<T: Send> Send for Ring<T> {}

impl<T> Ring<T> {
    /// A ring holding up to `capacity` values (rounded up to a power of two).
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        Ring {
            slots: (0..capacity)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect(),
            head: Padded(AtomicUsize::new(0)),
            tail: Padded(AtomicUsize::new(0)),
        }
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    /// Pushes a value; gives it back when the ring is full.
    ///
    /// Must only be called from the ring's single producer thread.
    pub(crate) fn push(&self, value: T) -> Result<(), T> {
        let tail = self.tail.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Acquire);
        if tail.wrapping_sub(head) == self.slots.len() {
            return Err(value);
        }
        let slot = &self.slots[tail & self.mask()];
        // SAFETY: only the producer writes, and `tail` is outside `head..tail`, so
        // the consumer does not read this slot until the release store below.
        unsafe { (*slot.get()).write(value) };
        self.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Pops the oldest value.
    ///
    /// Must only be called by the ring's single consumer (callers serialize it).
    pub(crate) fn pop(&self) -> Option<T> {
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let slot = &self.slots[head & self.mask()];
        // SAFETY: `head` is inside `head..tail`, so the producer initialized this
        // slot (published by its release store of `tail`) and will not touch it
        // again until the release store below hands it back.
        let value = unsafe { (*slot.get()).assume_init_read() };
        self.head.0.store(head.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Values waiting (approximate while the producer runs).
    pub(crate) fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Acquire);
        let head = self.head.0.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }
}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Ring;

    #[test]
    fn fills_drains_and_wraps() {
        let ring = Ring::new(3);
        for i in 0..4 {
            assert!(ring.push(i).is_ok());
        }
        assert_eq!(ring.push(9), Err(9), "capacity rounds up to 4");
        assert_eq!(ring.pop(), Some(0));
        assert!(ring.push(4).is_ok());
        let rest: Vec<i32> = std::iter::from_fn(|| ring.pop()).collect();
        assert_eq!(rest, [1, 2, 3, 4]);
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn one_producer_and_one_consumer_lose_nothing() {
        let ring = Arc::new(Ring::new(64));
        let producer = {
            let ring = Arc::clone(&ring);
            std::thread::spawn(move || {
                for i in 0..100_000u64 {
                    let mut v = i;
                    while let Err(back) = ring.push(v) {
                        v = back;
                        std::hint::spin_loop();
                    }
                }
            })
        };
        let mut next = 0u64;
        while next < 100_000 {
            if let Some(v) = ring.pop() {
                assert_eq!(v, next);
                next += 1;
            }
        }
        producer.join().expect("producer");
    }

    #[test]
    fn drops_values_left_in_the_ring() {
        let value = Arc::new(());
        let ring = Ring::new(4);
        assert!(ring.push(Arc::clone(&value)).is_ok());
        drop(ring);
        assert_eq!(Arc::strong_count(&value), 1);
    }
}
