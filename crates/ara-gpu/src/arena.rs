//! Generation checked, device-scoped identities without reference counting.
use crate::{GpuError, Result};
use std::{
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ARENA: AtomicU64 = AtomicU64::new(1);

/// An opaque typed identity. A handle cannot keep a GPU allocation alive.
pub struct Handle<T> {
    slot: u32,
    generation: u64,
    owner: u64,
    kind: PhantomData<fn() -> T>,
}
impl<T> Copy for Handle<T> {}
impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.owner, self.slot, self.generation) == (other.owner, other.slot, other.generation)
    }
}
impl<T> Eq for Handle<T> {}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Handle")
            .field(&self.owner)
            .field(&self.slot)
            .field(&self.generation)
            .finish()
    }
}

struct Slot<V> {
    generation: u64,
    value: Option<V>,
}
/// Backend-owned storage. Removing a value invalidates every old handle.
pub struct Arena<T, V> {
    owner: u64,
    slots: Vec<Slot<V>>,
    free: Vec<u32>,
    kind: PhantomData<fn() -> T>,
}
impl<T, V> Default for Arena<T, V> {
    fn default() -> Self {
        Self {
            owner: NEXT_ARENA.fetch_add(1, Ordering::Relaxed),
            slots: Vec::new(),
            free: Vec::new(),
            kind: PhantomData,
        }
    }
}
impl<T, V> Arena<T, V> {
    /// Insert a newly owned value.
    pub fn insert(&mut self, value: V) -> Result<Handle<T>> {
        let slot = match self.free.pop() {
            Some(slot) => slot,
            None => {
                let slot = u32::try_from(self.slots.len())
                    .map_err(|_| GpuError::Contract("Resource capacity exceeded".into()))?;
                self.slots.push(Slot {
                    generation: 1,
                    value: None,
                });
                slot
            }
        };
        let entry = &mut self.slots[slot as usize];
        entry.value = Some(value);
        Ok(Handle {
            slot,
            generation: entry.generation,
            owner: self.owner,
            kind: PhantomData,
        })
    }
    /// Borrow only if both generation and arena identity match.
    pub fn get(&self, handle: Handle<T>) -> Result<&V> {
        if handle.owner != self.owner {
            return Err(GpuError::StaleHandle);
        }
        self.slots
            .get(handle.slot as usize)
            .filter(|slot| slot.generation == handle.generation)
            .and_then(|slot| slot.value.as_ref())
            .ok_or(GpuError::StaleHandle)
    }
    /// Mutably borrow a live value.
    pub fn get_mut(&mut self, handle: Handle<T>) -> Result<&mut V> {
        if handle.owner != self.owner {
            return Err(GpuError::StaleHandle);
        }
        self.slots
            .get_mut(handle.slot as usize)
            .filter(|slot| slot.generation == handle.generation)
            .and_then(|slot| slot.value.as_mut())
            .ok_or(GpuError::StaleHandle)
    }
    /// Remove ownership, keeping exhausted generations permanently unavailable.
    pub fn remove(&mut self, handle: Handle<T>) -> Result<V> {
        self.get(handle)?;
        let slot = &mut self.slots[handle.slot as usize];
        let value = slot.value.take().ok_or(GpuError::StaleHandle)?;
        if let Some(generation) = slot.generation.checked_add(1) {
            slot.generation = generation;
            self.free.push(handle.slot);
        }
        Ok(value)
    }
    /// Iterate over live owned values, used for conservative bindless reachability.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots.iter_mut().filter_map(|slot| slot.value.as_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    enum Tag {}
    #[test]
    fn rejects_stale_and_foreign_handles() {
        let mut arena = Arena::<Tag, _>::default();
        let first = arena.insert(7).unwrap();
        assert_eq!(arena.remove(first).unwrap(), 7);
        let second = arena.insert(8).unwrap();
        assert!(arena.get(first).is_err());
        assert_eq!(*arena.get(second).unwrap(), 8);
        let other = Arena::<Tag, i32>::default();
        assert!(other.get(second).is_err());
    }
}
