use crate::*;
use anyhow::{Result, ensure};

/// One requested physical upload; invalidate an old mapping before publication.
#[derive(Debug)]
pub struct Upload {
    /// Virtual page address.
    pub page: u32,
    /// Physical tile slot.
    pub slot: u32,
    /// Evicted address, when the pool is full.
    pub evicted: Option<u32>,
}
struct Entry {
    slot: u32,
    used: u32,
    born: u32,
}
/// Deterministic priority/LRU allocator with pinned mip tails and parent-before-
/// child publication. No GPU resource ownership or per-frame blocking waits.
pub struct Pool {
    entries: Vec<Option<Entry>>,
    free: Vec<u32>,
    frame: u32,
    capacity: u32,
    uploads: usize,
}
impl Pool {
    /// Reserves a byte budget and checks that every texture's mip tail fits.
    pub fn new(catalog: &Catalog, budget: u64, upload_pages: usize) -> Result<Self> {
        let capacity = u32::try_from((budget / PAGE_BYTES).min(catalog.pages.len() as u64))?;
        ensure!(
            u64::from(capacity) * PAGE_BYTES <= u64::from(u32::MAX),
            "texture pool exceeds 32-bit shader address range"
        );
        ensure!(
            capacity as usize >= catalog.pages.iter().filter(|p| p.pinned).count()
                && upload_pages > 0,
            "texture budget cannot hold pinned mip tails"
        );
        Ok(Self {
            entries: (0..catalog.pages.len()).map(|_| None).collect(),
            free: (0..capacity).rev().collect(),
            frame: 0,
            capacity,
            uploads: upload_pages,
        })
    }
    /// Plans a completed frame's timestamped feedback. Feedback is persistent:
    /// recent demand is protected for three frames, allowing readback latency.
    pub fn request(
        &mut self,
        catalog: &Catalog,
        frame: u32,
        feedback: &[u32],
    ) -> Result<Vec<Upload>> {
        ensure!(
            frame > self.frame && feedback.len() == self.entries.len(),
            "invalid texture feedback frame or shape"
        );
        self.frame = frame;
        let protected: Vec<_> = catalog
            .pages
            .iter()
            .zip(feedback)
            .map(|(p, &last)| p.pinned || (last > 0 && last.saturating_add(3) >= frame))
            .collect();
        for (entry, &last) in self.entries.iter_mut().zip(feedback) {
            if let Some(e) = entry {
                e.used = e.used.max(last);
            }
        }
        let mut requests: Vec<_> = catalog
            .pages
            .iter()
            .enumerate()
            .filter(|(id, _)| protected[*id] && self.entries[*id].is_none())
            .map(|(id, p)| {
                (
                    std::cmp::Reverse(u32::from(p.pinned)),
                    std::cmp::Reverse(p.mip),
                    std::cmp::Reverse(catalog.sources[p.texture].priority),
                    id as u32,
                )
            })
            .collect();
        requests.sort_unstable();
        // Ancestors remain protected for this whole batch even if their last
        // child is evicted. Fallback can lose at most one resident level/frame.
        let mut resident_children = vec![0u32; self.entries.len()];
        for (id, page) in catalog.pages.iter().enumerate() {
            if self.entries[id].is_some()
                && let Some(parent) = page.parent
            {
                resident_children[parent as usize] += 1;
            }
        }
        let mut uploads = Vec::new();
        for (_, _, _, page) in requests {
            if uploads.len() >= self.uploads {
                break;
            }
            let p = &catalog.pages[page as usize];
            if !p.pinned
                && p.parent.is_some_and(|id| {
                    self.entries[id as usize]
                        .as_ref()
                        .is_none_or(|e| e.born >= frame)
                })
            {
                continue;
            }
            let candidate = if self.free.is_empty() {
                self.entries
                    .iter()
                    .enumerate()
                    .filter_map(|(id, e)| e.as_ref().map(|e| (id, e)))
                    .filter(|(id, _)| {
                        !catalog.pages[*id].pinned
                            && resident_children[*id] == 0
                            && (!protected[*id]
                                || catalog.sources[catalog.pages[*id].texture].priority
                                    < catalog.sources[p.texture].priority)
                    })
                    .min_by_key(|(id, e)| {
                        (
                            catalog.sources[catalog.pages[*id].texture].priority,
                            e.used,
                            *id,
                        )
                    })
                    .map(|(id, e)| (e.slot, Some(id as u32)))
            } else {
                self.free.pop().map(|slot| (slot, None))
            };
            let Some((slot, evicted)) = candidate else {
                continue;
            };
            if let Some(id) = evicted {
                self.entries[id as usize] = None;
            }
            self.entries[page as usize] = Some(Entry {
                slot,
                used: frame,
                born: frame,
            });
            uploads.push(Upload {
                page,
                slot,
                evicted,
            });
            if let Some(parent) = p.parent {
                resident_children[parent as usize] += 1;
            }
        }
        Ok(uploads)
    }
    /// Number of allocated physical tile slots.
    pub fn resident(&self) -> u32 {
        self.capacity - self.free.len() as u32
    }
    /// Physical page pool bytes, including gutters.
    pub fn bytes(&self) -> u64 {
        u64::from(self.capacity) * PAGE_BYTES
    }
    /// Checked resident physical address, or None for a missing virtual tile.
    pub fn slot(&self, page: u32) -> Option<u32> {
        self.entries.get(page as usize)?.as_ref().map(|e| e.slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn higher_priority_replaces_only_lower_priority_leaf_pages() {
        let make =
            |priority| Source::rgba8([256, 256], vec![128; 256 * 256 * 4], true, priority).unwrap();
        let catalog = Catalog::new(vec![make(1), make(100)]).unwrap();
        let pinned = catalog.pages.iter().filter(|p| p.pinned).count() as u64;
        let mut pool = Pool::new(&catalog, (pinned + 4) * PAGE_BYTES, 64).unwrap();
        let mut feedback = vec![0; catalog.page_count()];
        pool.request(&catalog, 1, &feedback).unwrap();
        for (id, page) in catalog.pages.iter().enumerate() {
            if page.texture == 0 {
                feedback[id] = 2;
            }
        }
        pool.request(&catalog, 2, &feedback).unwrap();
        assert!(pool.slot(0).is_some());
        for (id, page) in catalog.pages.iter().enumerate() {
            if page.texture == 1 {
                feedback[id] = 3;
            }
        }
        pool.request(&catalog, 3, &feedback).unwrap();
        assert!(pool.slot(0).is_none());
        assert!(pool.slot(catalog.descriptors[1][2]).is_some());
        for (id, page) in catalog.pages.iter().enumerate() {
            if page.pinned {
                assert!(pool.slot(id as u32).is_some());
            }
        }
    }
    #[test]
    fn npot_edge_pages_reference_the_correct_parent_level() {
        let catalog = Catalog::new(vec![
            Source::rgba8([257, 129], vec![255; 257 * 129 * 4], true, 0).unwrap(),
        ])
        .unwrap();
        for page in &catalog.pages {
            if let Some(parent) = page.parent {
                assert_eq!(catalog.pages[parent as usize].mip, page.mip + 1);
                assert_eq!(catalog.pages[parent as usize].texture, page.texture);
            }
        }
    }
    #[test]
    fn budget_and_parent_first_publication() {
        let catalog = Catalog::new(vec![
            Source::rgba8([512, 512], vec![255; 512 * 512 * 4], true, 1).unwrap(),
        ])
        .unwrap();
        let mut pool = Pool::new(&catalog, PAGE_BYTES * 24, 64).unwrap();
        let feedback = vec![1; catalog.page_count()];
        let mut prior = vec![false; catalog.page_count()];
        for frame in 1..=4 {
            for upload in pool.request(&catalog, frame, &feedback).unwrap() {
                let page = &catalog.pages[upload.page as usize];
                if !page.pinned {
                    assert!(prior[page.parent.unwrap() as usize]);
                }
            }
            for (id, resident) in prior.iter_mut().enumerate() {
                *resident = pool.slot(id as u32).is_some();
            }
            assert!(pool.bytes() <= PAGE_BYTES * 24);
        }
        assert!(pool.slot(0).is_some());
    }
    #[test]
    fn npot_mips_include_edge_texels_and_wrapping_gutters() {
        let source = Source::rgba8(
            [3, 1],
            vec![0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255],
            false,
            0,
        )
        .unwrap();
        assert_eq!(source.levels[1], [85, 85, 85, 255]);
        let catalog = Catalog::new(vec![source]).unwrap();
        let page = catalog.page(0).unwrap();
        assert_eq!(
            &page[(TILE_SIZE * 4) as usize..(TILE_SIZE * 4 + 4) as usize],
            &[255; 4]
        );
    }
}
