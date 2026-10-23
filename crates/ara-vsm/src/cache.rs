use super::*;
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
struct Entry {
    slot: u32,
    generation: u64,
    last_used: u64,
    valid: bool,
}
/// Bounded deterministic LRU. Call only at a completed frame boundary.
pub struct Cache {
    capacity: u32,
    entries: BTreeMap<PageKey, Entry>,
    free: BTreeSet<u32>,
    generation: u64,
    frame: u64,
    revisions: BTreeMap<u64, u64>,
}
impl Cache {
    /// Allocates metadata; no GPU resources or hidden waits.
    pub fn new(capacity: u32) -> Result<Self> {
        ensure!(capacity > 0, "ARA_VSM_CAPACITY: empty physical page pool");
        Ok(Self {
            capacity,
            entries: BTreeMap::new(),
            free: (0..capacity).collect(),
            generation: 0,
            frame: 0,
            revisions: BTreeMap::new(),
        })
    }
    /// Advances an explicit frame index and invalidates changed light revisions.
    pub fn begin(&mut self, frame: u64, lights: &[Light]) -> Result<()> {
        ensure!(
            frame >= self.frame,
            "ARA_VSM_FRAME: nonmonotonic completed frame"
        );
        self.frame = frame;
        for l in lights {
            ensure!(l.vp.is_finite(), "ARA_VSM_LIGHT: nonfinite projection");
            if self
                .revisions
                .insert(l.id, l.revision)
                .is_some_and(|r| r != l.revision)
            {
                for (key, e) in &mut self.entries {
                    if key.light == l.id {
                        e.valid = false;
                    }
                }
            }
        }
        Ok(())
    }
    /// Marks pages touched by depth. Requests are deduplicated and ordered before
    /// allocation; pages already touched this frame cannot evict one another.
    pub fn request(&mut self, keys: impl IntoIterator<Item = PageKey>) -> Result<Vec<DirtyPage>> {
        let keys: BTreeSet<_> = keys.into_iter().collect();
        for key in &keys {
            if let Some(e) = self.entries.get_mut(key) {
                e.last_used = self.frame;
            }
        }
        let mut dirty = Vec::new();
        for key in keys {
            ensure!(
                !matches!(key.projection,Projection::Cube(face) if face>=6),
                "invalid cube face"
            );
            if !self.entries.contains_key(&key) {
                let slot = if let Some(slot) = self.free.pop_first() {
                    slot
                } else {
                    let victim = self
                        .entries
                        .iter()
                        .filter(|(_, e)| e.last_used < self.frame)
                        .min_by_key(|(k, e)| (e.last_used, **k))
                        .map(|(k, _)| *k);
                    let Some(victim) = victim else {
                        continue;
                    };
                    self.entries
                        .remove(&victim)
                        .expect("selected cache entry exists")
                        .slot
                };
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("VSM generation exhausted"))?;
                self.entries.insert(
                    key,
                    Entry {
                        slot,
                        generation: self.generation,
                        last_used: self.frame,
                        valid: false,
                    },
                );
            }
            let e = &self.entries[&key];
            if !e.valid {
                dirty.push(DirtyPage {
                    key,
                    slot: e.slot,
                    generation: e.generation,
                });
            }
        }
        Ok(dirty)
    }
    /// Publishes only the matching assignment after the page-render fence completes.
    pub fn complete(&mut self, page: DirtyPage) -> bool {
        if let Some(e) = self.entries.get_mut(&page.key)
            && e.generation == page.generation
            && e.slot == page.slot
        {
            e.valid = true;
            return true;
        }
        false
    }
    /// Returns a sampleable slot; unrendered assignments are never exposed.
    pub fn resident(&self, key: PageKey) -> Option<u32> {
        self.entries.get(&key).filter(|e| e.valid).map(|e| e.slot)
    }
    /// Invalidates affected page keys for old AND new caster bounds on movement.
    pub fn invalidate(&mut self, keys: impl IntoIterator<Item = PageKey>) {
        for key in keys {
            if let Some(e) = self.entries.get_mut(&key) {
                e.valid = false;
            }
        }
    }
    /// Allocated physical pool size; depth storage uses four bytes per texel.
    pub fn bytes(&self) -> u64 {
        u64::from(self.capacity) * u64::from(PAGE_SIZE).pow(2) * 4
    }
}
