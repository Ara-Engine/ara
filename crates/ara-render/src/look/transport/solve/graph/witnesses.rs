//! Intern repaired visibility sets with one compact override ID per link. Dead
//! revisions release their storage; repeated motion cannot grow a stamp history.
use super::*;

const ORIGINAL: u32 = u32::MAX;

struct Entry {
    stamps: Arc<[GeometryStamp]>,
    references: usize,
}

#[derive(Default)]
pub(super) struct RepairedWitnesses {
    overrides: Vec<u32>,
    entries: Vec<Option<Entry>>,
    by_set: BTreeMap<Arc<[GeometryStamp]>, u32>,
    free: Vec<u32>,
}

impl RepairedWitnesses {
    pub fn get(&self, link: u32) -> Option<&[GeometryStamp]> {
        let id = *self.overrides.get(link as usize)?;
        (id != ORIGINAL).then(|| {
            self.entries[id as usize]
                .as_ref()
                .expect("live witness override")
                .stamps
                .as_ref()
        })
    }

    pub fn clear(&mut self, link: u32) {
        let Some(id) = self.overrides.get_mut(link as usize) else {
            return;
        };
        let old = std::mem::replace(id, ORIGINAL);
        self.release(old);
    }

    pub fn set(&mut self, link: u32, stamps: Vec<GeometryStamp>, links: usize) -> Result<()> {
        if self.get(link) == Some(stamps.as_slice()) {
            return Ok(());
        }
        let id = if let Some(&id) = self.by_set.get(stamps.as_slice()) {
            let entry = self.entries[id as usize]
                .as_mut()
                .expect("interned witness entry");
            entry.references += 1;
            id
        } else {
            let id = if let Some(id) = self.free.pop() {
                id
            } else {
                let id = u32::try_from(self.entries.len())?;
                ensure!(id != ORIGINAL, "repair witness capacity exhausted");
                self.entries.push(None);
                id
            };
            let stamps: Arc<[GeometryStamp]> = stamps.into();
            // Both the ordered interning key and stable slot share one immutable
            // allocation, rather than duplicating the geometry stamp payload.
            self.by_set.insert(Arc::clone(&stamps), id);
            self.entries[id as usize] = Some(Entry {
                stamps,
                references: 1,
            });
            id
        };
        if self.overrides.is_empty() {
            self.overrides.resize(links, ORIGINAL);
        }
        let old = std::mem::replace(&mut self.overrides[link as usize], id);
        self.release(old);
        Ok(())
    }

    fn release(&mut self, id: u32) {
        if id == ORIGINAL {
            return;
        }
        let slot = &mut self.entries[id as usize];
        let entry = slot.as_mut().expect("released witness is live");
        entry.references -= 1;
        if entry.references == 0 {
            let entry = slot.take().expect("last witness reference");
            self.by_set.remove(entry.stamps.as_ref());
            self.free.push(id);
        }
    }

    pub fn resident_bytes(&self) -> usize {
        self.overrides.capacity() * 4
            + self.entries.capacity() * size_of::<Option<Entry>>()
            + self.free.capacity() * 4
            + self
                .by_set
                .keys()
                .map(|stamps| std::mem::size_of_val(stamps.as_ref()) + 16 + 80)
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_revisions_are_reclaimed_and_original_links_need_no_override() -> Result<()> {
        let stamp = |generation| GeometryStamp {
            key: GeometryKey {
                instance: crate::look::trace::InstanceKey {
                    slot: 4,
                    generation: 1,
                },
                primitive: 17,
            },
            generation,
        };
        let mut pool = RepairedWitnesses::default();
        for link in 0..1024 {
            pool.clear(link);
        }
        assert_eq!(pool.resident_bytes(), 0);
        for generation in 1..=300 {
            for link in 0..1024 {
                pool.set(link, vec![stamp(generation)], 1024)?;
                assert_eq!(pool.get(link), Some([stamp(generation)].as_slice()));
            }
            assert_eq!(pool.by_set.len(), 1);
            assert!(pool.entries.len() <= 2);
            assert!(pool.resident_bytes() < 5 * 1024);
        }
        for link in 0..1024 {
            pool.clear(link);
            assert!(pool.get(link).is_none());
        }
        assert!(pool.by_set.is_empty());
        assert!(pool.entries.iter().all(Option::is_none));
        Ok(())
    }
}
