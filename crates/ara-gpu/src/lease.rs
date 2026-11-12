//! A refcounted handle that keeps one [`GpuDevice`] at a fixed address.
use crate::GpuDevice;
use std::cell::UnsafeCell;
use std::sync::Arc;

/// One heap allocation shared by every [`Share`].
struct Slot<T> {
    value: UnsafeCell<T>,
}

/// A handle to a `T` that stays at one address until the last handle drops.
struct Share<T> {
    inner: Arc<Slot<T>>,
}

impl<T> Share<T> {
    fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Slot {
                value: UnsafeCell::new(value),
            }),
        }
    }

    fn as_ptr(&self) -> *mut T {
        self.inner.value.get()
    }
}

impl<T> Clone for Share<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// SAFETY: `Slot<T>` is `Send` when `T` is. It is not `Sync` (`UnsafeCell`), so
// `Arc<Slot<T>>` is not automatically `Send`. A lease moves to another thread
// only as a value, with no live reference left behind, and recording stays on
// one thread. That is the same contract as `GpuDevice` itself.
unsafe impl<T: Send> Send for Share<T> {}

/// A handle to a [`GpuDevice`] that lives at one heap address.
///
/// Cloning shares the device. Moving a lease does not move the device. The
/// device is destroyed when the last lease drops, so a renderer that holds one
/// can outlive the owner that opened the device. One thread records;
/// leases are not `Sync`.
pub struct GpuLease {
    share: Share<GpuDevice>,
}

impl GpuLease {
    /// Takes ownership of `gpu` and puts it on the heap.
    pub fn new(gpu: GpuDevice) -> Self {
        Self {
            share: Share::new(gpu),
        }
    }

    /// The device. The reference is valid for as long as this lease is borrowed.
    /// Another lease of the same device must not be mutably borrowed at the same time.
    pub fn get(&self) -> &GpuDevice {
        // SAFETY: this lease keeps the allocation alive for the borrow of `self`.
        unsafe { &*self.share.as_ptr() }
    }

    /// Address of the device. Stable across clones and moves until the last lease drops.
    pub fn as_ptr(&self) -> *const GpuDevice {
        self.share.as_ptr()
    }

    /// Address of the device, for the one thread that records.
    ///
    /// Dereferencing it is sound while any lease of this device is alive and no
    /// other reference to the device is live.
    pub fn as_mut_ptr(&self) -> *mut GpuDevice {
        self.share.as_ptr()
    }
}

impl Clone for GpuLease {
    fn clone(&self) -> Self {
        Self {
            share: self.share.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Share;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Bomb(Arc<AtomicUsize>);

    impl Drop for Bomb {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn moving_and_dropping_one_owner_leaves_the_value_at_the_same_address() {
        let drops = Arc::new(AtomicUsize::new(0));
        let owner = Share::new(Bomb(Arc::clone(&drops)));
        let address = owner.as_ptr();
        let kept = owner.clone();
        let owner = owner;
        assert_eq!(owner.as_ptr(), address);
        assert_eq!(kept.as_ptr(), address);
        drop(owner);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(kept.as_ptr(), address);
        drop(kept);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
