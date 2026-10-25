//! Budgeted native BC images. Publication happens at completed frame boundaries.
use crate::NativeSource;
use anyhow::{Result, ensure};
use ara_asset_format::TextureFormat;
use ara_gpu::{Access, Buffer, Device, Image, Resource, TimelinePoint, vulkan::VulkanDevice};
use std::{collections::BTreeMap, sync::Arc};

#[cfg(test)]
mod benchmark;
mod plan;
mod reuse;
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

pub use ara_render::instanced::{TextureBinding as NativeBinding, TextureStats as NativeStats};

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
    borrowed: bool,
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

impl ara_render::instanced::Textures for NativeResidency {
    fn content_digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for (source, srgb) in self.content_keys() {
            digest.update(source);
            digest.update([u8::from(srgb)]);
        }
        digest.finalize().into()
    }
    fn initialize(&mut self, gpu: &mut VulkanDevice) -> Result<NativeStats> {
        Self::initialize(self, gpu)
    }
    fn initialize_reusing(
        &mut self,
        gpu: &mut VulkanDevice,
        previous: &dyn ara_render::instanced::Textures,
    ) -> Result<NativeStats> {
        self.prepare_reusing(gpu, previous)
    }
    fn publish_reuse(&mut self, previous: &mut dyn ara_render::instanced::Textures) -> Result<()> {
        self.commit_reuse(previous)
    }
    fn refine(&mut self, gpu: &mut VulkanDevice) -> Result<NativeStats> {
        Self::refine(self, gpu)
    }
    fn binding(&self) -> Option<NativeBinding> {
        Self::binding(self)
    }
    fn visit_resources(&self, visit: &mut dyn FnMut(Resource, Access)) {
        for (resource, access) in self.resources() {
            visit(resource, access);
        }
    }
    fn destroy(self: Box<Self>, gpu: &mut VulkanDevice) -> Result<()> {
        (*self).destroy(gpu)
    }
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

    /// Linear RGBA texels (row-major) of a registered map, decoded on the CPU from
    /// its finest level of at least one whole block with both sides at most
    /// `max_side`, else its coarsest such level. Colour is linearised per texel
    /// for cooked sRGB views and wherever `decode` is set: a raw view whose
    /// material decodes sRGB in the shader (`BASE_COLOR_DECODE_SRGB` /
    /// `EMISSIVE_DECODE_SRGB`), so pass that material's bit for the slot. For
    /// one-time consumers on worker threads, such as probe GI bakes and emitter
    /// lights; residency uploads never decode. `None` for unknown IDs or
    /// undecodable levels.
    pub fn texels(
        &self,
        id: u32,
        max_side: u32,
        decode: bool,
    ) -> Option<([u32; 2], Vec<[f32; 4]>)> {
        let entry = self.entries.get(id as usize)?;
        let source = &entry.source;
        let size = |level: u32| source.mip(level).map(|m| m.size);
        let whole = |level: &u32| size(*level).is_some_and(|[w, h]| w >= 4 && h >= 4);
        let level = (0..source.mip_count())
            .filter(whole)
            .find(|&level| size(level).is_some_and(|[w, h]| w.max(h) <= max_side))
            .or_else(|| (0..source.mip_count()).rev().find(whole))
            .unwrap_or(0);
        let mip = source.mip(level)?;
        let [w, h] = mip.size.map(|v| v as usize);
        let mut pixels = vec![0u32; w * h];
        match source.format() {
            TextureFormat::Bc4 => texture2ddecoder::decode_bc4(mip.bytes, w, h, &mut pixels),
            TextureFormat::Bc5 => texture2ddecoder::decode_bc5(mip.bytes, w, h, &mut pixels),
            TextureFormat::Bc7 | TextureFormat::Bc7Srgb => {
                texture2ddecoder::decode_bc7(mip.bytes, w, h, &mut pixels)
            }
            _ => return None,
        }
        .ok()?;
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let texels = pixels
            .iter()
            .map(|p| {
                // Decoded texels are 0xAARRGGBB.
                let rgba = [(p >> 16) & 255, (p >> 8) & 255, p & 255, p >> 24];
                let mut texel = rgba.map(|v| v as f32 / 255.);
                if entry.srgb || decode {
                    for v in &mut texel[..3] {
                        *v = linear(*v);
                    }
                }
                texel
            })
            .collect();
        Some((mip.size, texels))
    }

    /// Mean linear RGBA of a registered map over its coarsest level of at least
    /// one whole block ([`Self::texels`], `decode` likewise).
    pub fn mean_texel(&self, id: u32, decode: bool) -> Option<[f32; 4]> {
        let (_, texels) = self.texels(id, 4, decode)?;
        let mut sum = [0f64; 4];
        for texel in &texels {
            for (sum, v) in sum.iter_mut().zip(texel) {
                *sum += f64::from(*v);
            }
        }
        let n = texels.len().max(1) as f64;
        Some(sum.map(|s| (s / n) as f32))
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
            if let Some(resident) = entry.resident
                && !resident.borrowed
            {
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
