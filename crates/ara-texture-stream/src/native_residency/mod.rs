//! Budgeted native BC images. Publication happens at completed frame boundaries.
use crate::NativeSource;
use anyhow::{Result, ensure};
use ara_asset_format::TextureFormat;
use ara_gpu::{Access, Buffer, Device, Image, Resource, TimelinePoint, vulkan::VulkanDevice};
use std::{collections::BTreeMap, sync::Arc};

#[cfg(test)]
mod benchmark;
mod plan;
#[cfg(test)]
mod tests;
mod upload;

/// Transfer-function interpretation of compressed material blocks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum NativeView {
    /// Honor the cooked format; BC7 sRGB decodes before hardware filtering.
    #[default]
    Cooked,
    /// Sample raw UNORM, including BC7 sRGB payloads. Graph color slots decode
    /// after filtering; graph data slots retain the raw sampled value.
    Raw,
}

/// Stable texture identity within one native resident set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTextureId(u32);

impl NativeTextureId {
    /// Material-table index, independent of the current physical image or mip tail.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// Native material texture limits, separate from the RGBA virtual page pool.
#[derive(Debug, Clone, Copy)]
pub struct NativeConfig {
    /// Maximum published compressed mip-tail payload; excludes temporary uploads.
    pub resident_bytes: u64,
    /// Upload target per refinement. One indivisible tail can exceed this target.
    pub upload_bytes: u64,
}

/// GPU table consumed by native material sampling. Declare the owner's resources
/// in every sampling pass and keep the resident set alive until those passes finish.
#[derive(Debug, Clone, Copy)]
pub struct NativeBinding {
    /// Bindless storage-buffer index containing 32-byte native texture rows.
    pub buffer_index: u32,
    /// Number of initialized material texture IDs in the table.
    pub textures: u32,
}

/// Payload and refinement measurements; no pixel decoding or atlas memory is hidden.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct NativeStats {
    /// Distinct registered cooked payload/view identities.
    pub textures: usize,
    /// Published native compressed image payload.
    pub resident_bytes: u64,
    /// Payload of the requested levels before budget-driven coarsening.
    pub requested_bytes: u64,
    /// Images whose published first mip still differs from the budgeted plan.
    pub pending_textures: usize,
    /// Compressed bytes uploaded by the most recent preparation/refinement.
    pub uploaded_bytes: u64,
    /// GPU time for the most recent upload batch, excluding host preparation.
    pub upload_gpu_ms: f64,
}

// ABI: slot, resident first mip, original width/height, encoding, resident levels,
// then two reserved words. Encoding 1 is BC4, 2 is BC5 XY, and 0 is BC7 colour/data.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Row([u32; 8]);

#[derive(Clone, Copy)]
struct Resident {
    image: Image,
    row: Row,
    bytes: u64,
}

struct Entry {
    source: Arc<NativeSource>,
    srgb: bool,
    desired: u32,
    priority: f32,
    resident: Option<Resident>,
}

struct Table {
    buffer: Buffer,
    binding: NativeBinding,
}

/// Shared native material residency for a device. Register at content publication,
/// initialize once, then refine at completed frame boundaries. Every registered map
/// retains an actual mip tail; budget pressure never folds textures into factors.
pub struct NativeResidency {
    config: NativeConfig,
    entries: Vec<Entry>,
    keys: BTreeMap<([u8; 32], bool), NativeTextureId>,
    plan: Vec<u32>,
    rows: Vec<Row>,
    dirty: bool,
    table: Option<Table>,
    last: Option<TimelinePoint>,
    stats: NativeStats,
}

impl NativeResidency {
    /// Address-independent cooked payload/view identities for capture provenance.
    /// Hash these only when publishing content, never in per-frame presentation.
    pub fn content_keys(&self) -> impl Iterator<Item = ([u8; 32], bool)> + '_ {
        self.keys.keys().copied()
    }

    /// Creates another view's resident set with the same stable IDs and shared
    /// compressed source bytes. No GPU resources or in-flight state are copied.
    /// Use only at content/view creation boundaries, never per frame.
    pub fn fork_sources(&self, config: NativeConfig) -> Result<Self> {
        let mut result = Self::new(config)?;
        result.keys = self.keys.clone();
        result.entries = self
            .entries
            .iter()
            .map(|entry| Entry {
                source: Arc::clone(&entry.source),
                srgb: entry.srgb,
                desired: entry.desired,
                priority: entry.priority,
                resident: None,
            })
            .collect();
        Ok(result)
    }

    /// Creates a resident set without allocating GPU resources.
    pub fn new(config: NativeConfig) -> Result<Self> {
        ensure!(
            config.resident_bytes > 0 && config.upload_bytes > 0,
            "invalid native texture budget"
        );
        Ok(Self {
            config,
            entries: Vec::new(),
            keys: BTreeMap::new(),
            plan: Vec::new(),
            rows: Vec::new(),
            dirty: true,
            table: None,
            last: None,
            stats: NativeStats::default(),
        })
    }

    /// Registers a validated source once per content generation. `key` must hash
    /// the complete cooked payload, including encoding; identical maps then share
    /// one GPU allocation even when referenced by different model generations.
    pub fn register(
        &mut self,
        key: [u8; 32],
        source: Arc<NativeSource>,
    ) -> Result<NativeTextureId> {
        self.register_view(key, source, NativeView::Cooked)
    }

    /// Registers an explicit sampled interpretation without decoding/re-encoding
    /// blocks. Dedup includes the effective view format so graph UNORM and glTF
    /// sRGB cannot share an incompatible image interpretation.
    pub fn register_view(
        &mut self,
        key: [u8; 32],
        source: Arc<NativeSource>,
        view: NativeView,
    ) -> Result<NativeTextureId> {
        let srgb = view == NativeView::Cooked && source.format() == TextureFormat::Bc7Srgb;
        let key = (key, srgb);
        if let Some(&id) = self.keys.get(&key) {
            let current = &self.entries[id.0 as usize].source;
            ensure!(
                current.size() == source.size()
                    && current.format() == source.format()
                    && current.mip_count() == source.mip_count(),
                "native texture key reused for another source"
            );
            return Ok(id);
        }
        let id = NativeTextureId(u32::try_from(self.entries.len())?);
        self.entries.push(Entry {
            source,
            srgb,
            desired: 0,
            priority: 1.0,
            resident: None,
        });
        self.keys.insert(key, id);
        self.dirty = true;
        Ok(id)
    }

    /// Sets aggregated demand for a shared map. Callers combine the finest desired
    /// mip and greatest projected importance across its instances before calling.
    pub fn request(&mut self, id: NativeTextureId, first_mip: u32, priority: f32) -> Result<()> {
        let entry = self
            .entries
            .get_mut(id.0 as usize)
            .ok_or_else(|| anyhow::anyhow!("unknown native texture"))?;
        ensure!(
            first_mip < entry.source.mip_count() && priority.is_finite() && priority >= 0.0,
            "invalid native texture demand"
        );
        if entry.desired != first_mip || entry.priority != priority {
            entry.desired = first_mip;
            entry.priority = priority;
            self.dirty = true;
        }
        Ok(())
    }

    /// Publishes complete budgeted tails for new content. This upload boundary may
    /// exceed the refinement target so no new material observes an uninitialized map.
    pub fn initialize(&mut self, gpu: &mut VulkanDevice) -> Result<NativeStats> {
        self.prepare(gpu, true)
    }

    /// Refines existing images within the upload target, preserving old bindings on
    /// failure. No I/O, decompression, mip synthesis or per-frame source hashing occurs.
    pub fn refine(&mut self, gpu: &mut VulkanDevice) -> Result<NativeStats> {
        ensure!(
            self.entries.len()
                == self
                    .table
                    .as_ref()
                    .map_or(0, |t| t.binding.textures as usize),
            "new native textures require initialization at a content boundary"
        );
        self.prepare(gpu, false)
    }

    /// Last fully published texture table, if the resident set contains any images.
    pub fn binding(&self) -> Option<NativeBinding> {
        self.table.as_ref().map(|table| table.binding)
    }

    /// Current residency and the most recent upload measurements.
    pub fn stats(&self) -> NativeStats {
        self.stats
    }

    /// Original finest-level size and currently published first mip of a texture.
    pub fn resident_mip(&self, id: NativeTextureId) -> Option<([u32; 2], u32)> {
        let entry = self.entries.get(id.0 as usize)?;
        Some((entry.source.size(), entry.resident?.row.0[1]))
    }

    /// Direct and bindless resources reached by native texture sampling.
    pub fn resources(&self) -> impl Iterator<Item = (Resource, Access)> + '_ {
        self.table
            .iter()
            .map(|t| (Resource::Buffer(t.buffer), Access::ShaderRead))
            .chain(self.entries.iter().filter_map(|entry| {
                entry
                    .resident
                    .map(|r| (Resource::Image(r.image), Access::ShaderRead))
            }))
    }

    /// Records the last submission that can sample these images. Preparation refuses
    /// to replace resources until that submission has completed.
    pub fn submitted(&mut self, point: TimelinePoint) {
        self.last = Some(point);
    }

    /// Retires native images and the binding table before the device is destroyed.
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for entry in self.entries {
            if let Some(resident) = entry.resident {
                gpu.retire(Resource::Image(resident.image))?;
            }
        }
        if let Some(table) = self.table {
            gpu.retire(Resource::Buffer(table.buffer))?;
        }
        gpu.collect()?;
        Ok(())
    }
}
