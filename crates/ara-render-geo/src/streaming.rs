//! Completed-boundary LRU ownership for independently decoded cluster pages.
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
struct Entry {
    slot: u32,
    used: u64,
    pinned: bool,
}
/// Deterministic page-pool allocator. A host publishes GPU page-table changes only
/// after uploads complete, and invalidates a mapping before reusing its slot.
pub struct PagePool {
    entries: BTreeMap<u32, Entry>,
    free: BTreeSet<u32>,
    frame: u64,
}
/// One physical upload with an optional mapping to invalidate first.
#[derive(Debug, PartialEq, Eq)]
pub struct Upload {
    /// Requested virtual cluster page.
    pub page: u32,
    /// Reusable physical slot.
    pub slot: u32,
    /// Old page occupying that slot; remove its mapping before replacement.
    pub evicted: Option<u32>,
}

use super::*;
const UPLOAD_PAGES: usize = 16;
const PACKET_WORDS: usize = 4 + 512;
pub(crate) struct Residency {
    pub table: Buffer,
    pub readback: Buffer,
    pub upload: Buffer,
    pub table_slot: u32,
    pub upload_slot: u32,
    pub pool: PagePool,
    pub floor: u32,
    pub pending: u32,
    pub capacity: u32,
    pub revision: u64,
    frame: u64,
    identity: [u8; 32],
}
impl Residency {
    pub fn new(gpu: &mut GpuDevice, geometry: &Geometry, capacity: u32) -> Result<(Self, Vec<u8>)> {
        let capacity = capacity.min(u32::try_from(geometry.clusters.len())?);
        let roots: Vec<_> = geometry
            .clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.parent == ara_geo::ROOT)
            .map(|(i, _)| i as u32)
            .collect();
        let mut pool = PagePool::new(capacity, &roots)?;
        let mut order: Vec<_> = geometry
            .clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.parent != ara_geo::ROOT)
            .map(|(i, c)| (std::cmp::Reverse(c.level), i as u32))
            .collect();
        order.sort_unstable();
        let budget = capacity as usize - roots.len();
        pool.request(1, order.into_iter().take(budget).map(|(_, id)| id))?;
        let mut pages = vec![0; capacity as usize * 2048];
        let mut table = vec![u32::MAX; geometry.clusters.len()];
        for (page, e) in &pool.entries {
            let decoded = geometry.cluster(*page)?;
            let offset = e.slot as usize * 2048;
            let vertices = bytemuck::cast_slice(&decoded.vertices);
            pages[offset..offset + vertices.len()].copy_from_slice(vertices);
            pages[offset + 1024..offset + 1024 + decoded.triangles.len()]
                .copy_from_slice(&decoded.triangles);
            table[*page as usize] = e.slot;
        }
        let floor = floor(geometry, &pool);
        let table = super::init::upload(
            gpu,
            bytemuck::cast_slice(&table),
            ara_gpu::vk::BufferUsageFlags::empty(),
        )?;
        let table_slot = gpu.storage_buffer(table)?.index;
        let readback = gpu.create_buffer(
            (geometry.clusters.len() + 8) as u64 * 4,
            ara_gpu::vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let upload = gpu.create_buffer(
            (UPLOAD_PAGES * PACKET_WORDS * 4) as u64,
            ara_gpu::vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let upload_slot = gpu.storage_buffer(upload)?.index;
        Ok((
            Self {
                table,
                readback,
                upload,
                table_slot,
                upload_slot,
                pool,
                floor,
                pending: 0,
                capacity,
                revision: 0,
                frame: 1,
                identity: geometry.identity()?,
            },
            pages,
        ))
    }
}
fn floor(geometry: &Geometry, pool: &PagePool) -> u32 {
    geometry
        .clusters
        .iter()
        .enumerate()
        .filter(|(id, _)| pool.slot(*id as u32).is_none())
        .map(|(_, c)| c.level + 1)
        .max()
        .unwrap_or(0)
}
impl GeometryView {
    /// Services prior-frame GPU feedback after its frame slot completes. Decode
    /// on a background extraction worker; record uploads before publishing the
    /// next view. Transfers are bounded to 16 independently compressed pages.
    pub fn prepare_streaming(&mut self, gpu: &mut GpuDevice, geometry: &Geometry) -> Result<u32> {
        ensure!(
            geometry.identity()? == self.residency.identity,
            "stream source content identity differs"
        );
        ensure!(
            self.residency.pending == 0,
            "submit queued page uploads before preparing another batch"
        );
        if let Some(p) = self.last {
            ensure!(
                gpu.completed()?[p.queue.index()] >= p.value,
                "stream feedback is still in flight"
            );
        } else {
            return Ok(0);
        }
        let feedback = gpu.read(self.residency.readback)?;
        let mut decoded = BTreeMap::new();
        for (id, bytes) in feedback
            .as_chunks::<4>()
            .0
            .iter()
            .take(geometry.clusters.len())
            .enumerate()
        {
            if u32::from_le_bytes(*bytes) != 0 && self.residency.pool.slot(id as u32).is_none() {
                decoded.insert(id as u32, geometry.cluster(id as u32)?);
                if decoded.len() == UPLOAD_PAGES {
                    break;
                }
            }
        }
        self.residency.frame += 1;
        let protected = geometry
            .clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.level >= self.residency.floor)
            .map(|(id, _)| id as u32);
        let updates = self.residency.pool.request(
            self.residency.frame,
            protected.chain(decoded.keys().copied()),
        )?;
        let mut packets = vec![0u32; updates.len() * PACKET_WORDS];
        for (i, u) in updates.iter().enumerate() {
            let page = &decoded[&u.page];
            let words = &mut packets[i * PACKET_WORDS..(i + 1) * PACKET_WORDS];
            words[..4].copy_from_slice(&[u.page, u.slot, u.evicted.unwrap_or(u32::MAX), 0]);
            let bytes = bytemuck::cast_slice_mut::<u32, u8>(&mut words[4..]);
            let vertices = bytemuck::cast_slice(&page.vertices);
            bytes[..vertices.len()].copy_from_slice(vertices);
            bytes[1024..1024 + page.triangles.len()].copy_from_slice(&page.triangles);
        }
        if !packets.is_empty() {
            gpu.upload(self.residency.upload, bytemuck::cast_slice(&packets))?;
        }
        self.residency.floor = floor(geometry, &self.residency.pool);
        self.residency.pending = updates.len() as u32;
        if !updates.is_empty() {
            self.residency.revision += 1;
        }
        Ok(self.residency.pending)
    }
}
impl PagePool {
    /// Number of assigned physical slots, including pinned roots.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether the pool has no assignments.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Reserves a bounded pool and pins all root pages permanently.
    pub fn new(capacity: u32, roots: &[u32]) -> Result<Self> {
        let roots: BTreeSet<_> = roots.iter().copied().collect();
        ensure!(
            capacity > 0 && roots.len() <= capacity as usize,
            "ARA_GEO_POOL: roots exceed page budget"
        );
        let mut pool = Self {
            entries: BTreeMap::new(),
            free: (0..capacity).collect(),
            frame: 0,
        };
        for page in roots {
            let slot = pool.free.pop_first().expect("validated root capacity");
            pool.entries.insert(
                page,
                Entry {
                    slot,
                    used: 0,
                    pinned: true,
                },
            );
        }
        Ok(pool)
    }
    /// Returns immutable root mappings for initial uploads.
    pub fn roots(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.entries
            .iter()
            .filter(|(_, e)| e.pinned)
            .map(|(&p, e)| (p, e.slot))
    }
    /// Services deduplicated GPU feedback at an explicit completed frame. Current
    /// requests are protected from eviction within the same batch.
    pub fn request(
        &mut self,
        frame: u64,
        feedback: impl IntoIterator<Item = u32>,
    ) -> Result<Vec<Upload>> {
        ensure!(frame > self.frame, "page feedback frame must increase");
        self.frame = frame;
        let pages: BTreeSet<_> = feedback.into_iter().collect();
        for p in &pages {
            if let Some(e) = self.entries.get_mut(p) {
                e.used = frame;
            }
        }
        let mut result = Vec::new();
        for page in pages {
            if self.entries.contains_key(&page) {
                continue;
            }
            let (slot, evicted) = if let Some(slot) = self.free.pop_first() {
                (slot, None)
            } else {
                let victim = self
                    .entries
                    .iter()
                    .filter(|(_, e)| !e.pinned && e.used < frame)
                    .min_by_key(|(p, e)| (e.used, **p))
                    .map(|(&p, _)| p);
                let Some(victim) = victim else {
                    continue;
                };
                let e = self
                    .entries
                    .remove(&victim)
                    .expect("selected victim exists");
                (e.slot, Some(victim))
            };
            self.entries.insert(
                page,
                Entry {
                    slot,
                    used: frame,
                    pinned: false,
                },
            );
            result.push(Upload {
                page,
                slot,
                evicted,
            });
        }
        Ok(result)
    }
    /// Resident physical slot after the host has committed its upload transaction.
    pub fn slot(&self, page: u32) -> Option<u32> {
        self.entries.get(&page).map(|e| e.slot)
    }
}
