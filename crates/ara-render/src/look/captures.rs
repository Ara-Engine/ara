//! Local reflection captures (shaders/look-captures.slang(h)): box-influenced,
//! box-projected octahedral radiance that the raster renderer bakes of its own lit
//! scene (six unjittered 90-degree faces of a dedicated view), GGX-prefiltered into
//! five roughness levels per capture in one RGBA16F atlas, and relit every frame by
//! the ratio of the ambient light at each capture point now to what it was at its
//! bake (id Tech style re-fit). Shading reads them in the High/Epic PSOs only.
//!
//! [`CaptureSpec`]s come from authored `ReflectionProbe` components or from
//! [`auto_place`]; `Renderer::set_reflection_captures` hands them over. [`Captures`]
//! plans the bakes: on new specs, a new content generation or a lighting change
//! (sun, local lights, sky, fog, probe GI arrival; rate-limited), never per frame.
//! Live views bake one step per frame (a face, or the reduce and prefilter), captures
//! and deterministic frames every due step at once, and only while probe GI is settled. A capture's previous bake stays in use
//! until its replacement lands in one prefilter dispatch.
use super::frame::LookFrame;
use crate::graph::{Graph, ResourceId, State};
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Queue, Resource, vk};
use glam::{Mat4, Quat, Vec3};

/// Prefiltered roughness levels per capture: 0, .25, .5, .75 and 1.
pub(crate) const LEVELS: u32 = 5;
/// Cube faces rendered per bake (+x, -x, +y, -y, +z, -z).
pub(crate) const FACES: u32 = 6;
/// `CaptureRecord` (look-captures.slangh) and the relight state per capture.
pub use ara_frame::look::captures::{
    auto_place, outside, place, CaptureSpec, MAX_CAPTURES, OPEN, RECORD_BYTES,
};
/// Per capture in the frame buffer: the record's copy, relight ratio and baseline.
const SLOT_BYTES: usize = 160;
/// 16x16 render tiles the frame buffer holds capture masks for (8192 squared).
const TILE_CAPACITY: usize = 512 * 512;
/// Dev-only shading modes from `ARA_CAPTURE_DEBUG` (read once; look-captures.slangh
/// `CAPTURE_DEBUG_*`): `mirror` shows every surface as a mirror of the captures alone
/// (roughness zero, no probe, sky or screen-space reflection); `only` keeps normal
/// shading but drops screen-space reflections where captures cover; `infinite` drops
/// the box parallax (orientation checks). Combine with commas.
fn debug_modes() -> u32 {
    static MODES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MODES.get_or_init(|| {
        let value = std::env::var("ARA_CAPTURE_DEBUG").unwrap_or_default();
        value.split(',').fold(0, |modes, mode| match mode.trim() {
            "mirror" => modes | 1,
            "only" => modes | 2,
            "infinite" => modes | 4,
            _ => modes,
        })
    })
}

/// Frames after a bake before lighting changes may start another (content changes
/// and new specs never wait).
const COOLDOWN: u32 = 60;
/// Live frames a bake waits for streamed textures before baking anyway.
const TEXTURE_WAIT: u32 = 240;
/// Extra bake rounds after automatic captures bake: each round sees the previous
/// one (bakes shade with the ready captures), so metal facing metal reflects more
/// than probe light. Without them a mirrored corridor of metals stays dark.
const BOUNCES: u32 = 2;

/// First row of roughness level `k`'s bordered tile in a column whose level 0 has
/// `n` interior texels (look-captures.slangh `captureLevelY`).
pub(crate) fn level_y(n: u32, k: u32) -> u32 {
    2 * n - ((2 * n) >> k) + 2 * k
}

/// Atlas columns: one per record (x, level-0 interior texels), then the bake's
/// scratch column, sized for the largest.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    pub columns: Vec<(u32, u32)>,
    pub scratch: u32,
    pub size: [u32; 2],
}

impl Layout {
    fn new(resolutions: &[u32]) -> Self {
        let mut x = 0;
        let columns = resolutions
            .iter()
            .map(|&n| {
                let column = (x, n);
                x += n + 2;
                column
            })
            .collect();
        let largest = resolutions.iter().copied().max().unwrap_or(64);
        Self {
            columns,
            scratch: x,
            size: [x + largest + 2, level_y(largest, LEVELS)],
        }
    }
}

/// What the baked captures' lighting depends on (`Captures::plan`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Lighting {
    /// The published content generation (`Content::base_hash`).
    pub content: String,
    /// Direction towards the sun and its illuminance (lux).
    pub sun: [f32; 6],
    /// The authored sky and fog (grading and lens effects come after the bake).
    pub look: (Option<super::Sky>, Option<super::Fog>),
    /// Hash of the local lights.
    pub lights: u64,
    /// The settled probe GI pool: twice its probes, plus one once its finer
    /// lattices landed (zero: none).
    pub gi: u32,
}

impl Lighting {
    /// Whether light changed enough for a re-bake: the sun turned by half a degree
    /// or its illuminance changed by 2% (the relight ratio covers the rest), or the
    /// sky, fog, local lights or probe pool changed.
    fn differs(&self, other: &Self) -> bool {
        let [a, b] = [self, other].map(|l| Vec3::new(l.sun[0], l.sun[1], l.sun[2]));
        let [x, y] = [self, other].map(|l| Vec3::new(l.sun[3], l.sun[4], l.sun[5]).length());
        a.normalize_or_zero().dot(b.normalize_or_zero()) < 0.99996
            || (x - y).abs() > 0.02 * x.max(y)
            || self.look != other.look
            || self.lights != other.lights
            || self.gi != other.gi
    }
}

/// One bake step (`Captures::plan`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Work {
    /// Render face `face` of record `capture` from `position` (a square view of
    /// `resolution` pixels) and project it into the scratch column.
    Face {
        capture: usize,
        face: u32,
        position: Vec3,
        resolution: u32,
    },
    /// Reduce the scratch column and prefilter it into the record's column.
    Finish { capture: usize, resolution: u32 },
}

/// The atlas and buffers of the current layout.
struct CaptureGpu {
    atlas: Image,
    sampled: u32,
    storage: u32,
    atlas_state: State,
    records: Buffer,
    records_index: u32,
    /// Device-local, persistent (look-captures.slangh): record copies (rewritten
    /// when the records change), relight state and the per-tile capture masks
    /// (every frame before lighting).
    frame: Buffer,
    frame_index: u32,
    frame_access: Access,
}

impl CaptureGpu {
    fn new(gpu: &mut GpuDevice, size: [u32; 2]) -> Result<Self> {
        let atlas = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R16G16B16A16_SFLOAT,
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        let sampled = gpu.sampled_image(atlas, false)?.index;
        let storage = gpu.storage_image(atlas)?.index;
        let records = gpu.create_buffer(
            (MAX_CAPTURES * RECORD_BYTES) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let records_index = gpu.storage_buffer(records)?.index;
        // Its baselines start undefined: the first relight clears them (`reset`).
        let frame = gpu.create_buffer(
            (MAX_CAPTURES * SLOT_BYTES + TILE_CAPACITY * 4) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            false,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        Ok(Self {
            atlas,
            sampled,
            storage,
            atlas_state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
            records,
            records_index,
            frame,
            frame_index,
            frame_access: Access::Undefined,
        })
    }

    fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.atlas))?;
        gpu.retire(Resource::Buffer(self.records))?;
        gpu.retire(Resource::Buffer(self.frame))?;
        Ok(())
    }
}

/// Graph identities of the captures this frame relights and shading reads.
pub(crate) struct CaptureIds {
    pub atlas: ResourceId,
    pub records: ResourceId,
    pub frame: ResourceId,
}

/// The renderer's reflection captures: specs, their atlas and the bake plan.
#[derive(Default)]
pub(crate) struct Captures {
    /// Specs as the host set them (`Renderer::set_reflection_captures`).
    specs: Vec<CaptureSpec>,
    /// Records: specs in blending order (priority, then smaller first), capped by
    /// the quality.
    records: Vec<CaptureSpec>,
    layout: Option<Layout>,
    gpu: Option<CaptureGpu>,
    /// Per record: needs a (re)bake.
    pending: Vec<bool>,
    /// Per record: a bake of the current atlas landed (shading may read it).
    ready: Vec<bool>,
    /// The record being baked and its next face (live views).
    progress: Option<(usize, u32)>,
    /// What the last requested bakes are lit by.
    lighting: Option<Lighting>,
    /// Records whose bake landed since the last relight pass (bit per record).
    rebaseline: u32,
    /// Records whose baseline the next relight clears (a new atlas, a moved capture).
    reset: u32,
    /// Records whose bake the next relight shows at once instead of fading it in over
    /// the probe reflections (a bake of every due step at once, or a restored image).
    instant: u32,
    /// The last plan baked every due step at once (captures, deterministic frames).
    sync: bool,
    /// Records changed since the frame buffer's copy: the next relight copies them
    /// (`copy_records`).
    copy: bool,
    cooldown: u32,
    waited: u32,
    /// Bake frames are rendering (a capture view is swapped in).
    pub baking: bool,
    /// A bake failed: captures stay off until new specs arrive.
    failed: bool,
    /// The specs changed since the last plan built records from them.
    unplanned: bool,
    /// Bounce rounds left after the current round ([`BOUNCES`]).
    bounces: u32,
    /// Per record: bakes completed into its column (explicit records' receipts).
    bakes: Vec<u64>,
    /// Explicit records' columns must be restored from retained images (a new atlas,
    /// or another probe moved into a column): `Renderer` uploads them.
    restore: bool,
}

/// A published explicit capture as read back from the atlas
/// (`Renderer::take_reflection_capture_images`, `Renderer::load_reflection_capture`):
/// its column, borders and all five prefiltered levels, row-major RGBA16F.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureImage {
    /// The probe's key ([`CaptureSpec::key`]).
    pub key: u64,
    /// Which of the column's bakes it is (`BakeStatus::bakes`); 0 for a loaded image.
    pub bake: u64,
    /// Level-0 interior texels per side.
    pub resolution: u32,
    /// Texel rows and row width of `bytes` (`resolution + 2` wide).
    pub extent: [u32; 2],
    /// `extent[0] * extent[1]` RGBA16F texels.
    pub bytes: Vec<u8>,
}

impl CaptureImage {
    /// The column extent of a capture of `resolution` texels: width, height.
    pub fn extent_for(resolution: u32) -> [u32; 2] {
        [resolution + 2, level_y(resolution, LEVELS)]
    }
}

/// One record's bake state (`Captures::status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakeStatus {
    /// A bake is requested and not yet published.
    pub pending: bool,
    /// Steps done while this record is the one baking (0-5 faces rendered; 6: the
    /// faces are done and the reduce and prefilter publish next); `None` while it
    /// waits its turn or nothing is pending.
    pub step: Option<u32>,
    /// Bake steps per capture (six faces and the prefilter).
    pub steps: u32,
    /// The column holds a published image shading reads.
    pub ready: bool,
    /// Bakes published into the column so far.
    pub bakes: u64,
    /// Level-0 texels per side of the column.
    pub resolution: u32,
}

/// Probe GI and texture residency are settled enough to bake (`Captures::plan`).
pub(crate) struct Settled {
    pub gi: bool,
    pub textures: bool,
}

impl Captures {
    /// Replaces the specs (validated); unchanged specs keep every bake.
    pub(crate) fn set_specs(&mut self, specs: &[CaptureSpec]) -> Result<()> {
        for spec in specs {
            spec.validate()?;
        }
        if self.specs != specs {
            self.specs = specs.to_vec();
            self.failed = false;
            self.unplanned = true;
        }
        Ok(())
    }

    /// Stops baking and reading captures after a failed bake, until new specs.
    pub(crate) fn fail(&mut self) {
        self.failed = true;
        self.progress = None;
    }

    /// Fits the atlas to the specs and quality, notes what invalidates bakes and
    /// returns this frame's bake work: one step (a face or a finish) in live frames,
    /// every due step when `sync` (captures, deterministic frames). Nothing bakes
    /// while probe GI converges, nor in live frames while textures stream (for at
    /// most `TEXTURE_WAIT` frames).
    pub(crate) fn plan(
        &mut self,
        gpu: &mut GpuDevice,
        quality: Option<super::ReflectionCaptures>,
        lighting: Lighting,
        settled: Settled,
        sync: bool,
    ) -> Result<Vec<Work>> {
        self.sync = sync;
        let Some(quality) = quality else {
            return Ok(Vec::new());
        };
        if self.failed || self.specs.is_empty() {
            return Ok(Vec::new());
        }
        self.unplanned = false;
        let mut records = self.specs.clone();
        records.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.volume().total_cmp(&b.volume()))
        });
        records.truncate((quality.captures as usize).min(MAX_CAPTURES));
        let resolutions: Vec<u32> = records
            .iter()
            .map(|r| match r.resolution {
                0 => quality.resolution,
                n => n,
            })
            .collect();
        let layout = Layout::new(&resolutions);
        if self.layout.as_ref() != Some(&layout) {
            if let Some(old) = self.gpu.take() {
                old.destroy(gpu)?;
            }
            self.layout = None;
            self.gpu = Some(CaptureGpu::new(gpu, layout.size)?);
            self.pending = records.iter().map(|r| !r.explicit).collect();
            self.bounces = BOUNCES;
            self.ready = vec![false; records.len()];
            self.bakes = vec![0; records.len()];
            self.restore = records.iter().any(|r| r.explicit);
            self.progress = None;
            self.rebaseline = 0;
            self.reset = u32::MAX;
            self.records = records;
            self.layout = Some(layout);
            self.upload_records(gpu)?;
        } else if self.records != records {
            for (i, (old, new)) in self.records.iter().zip(&records).enumerate() {
                if old == new {
                    continue;
                }
                if new.explicit {
                    // An explicit probe keeps its image (stale) until a requested bake
                    // replaces it; another probe moving into the column restores its own.
                    if old.key != new.key {
                        self.ready[i] = false;
                        self.pending[i] = false;
                        self.bakes[i] = 0;
                        self.restore = true;
                        if self.progress.is_some_and(|(c, _)| c == i) {
                            self.progress = None;
                        }
                    }
                    continue;
                }
                self.pending[i] = true;
                self.bounces = BOUNCES;
                if self.progress.is_some_and(|(c, _)| c == i) {
                    self.progress = None;
                }
                // A capture taken elsewhere is wrong until it is baked again.
                if old.capture_point() != new.capture_point() {
                    self.ready[i] = false;
                    self.reset |= 1 << i;
                }
            }
            self.records = records;
            self.upload_records(gpu)?;
        }
        // A converging probe volume (a capture reset, say) keeps the pool it had.
        let mut lighting = lighting;
        if !settled.gi
            && let Some(old) = &self.lighting
        {
            lighting.gi = old.gi;
        }
        let rebake = match &self.lighting {
            None => true,
            Some(old) if old.content != lighting.content => true,
            Some(old) => old.differs(&lighting) && (self.cooldown == 0 || sync),
        };
        if rebake {
            for (pending, record) in self.pending.iter_mut().zip(&self.records) {
                *pending |= !record.explicit;
            }
            self.bounces = BOUNCES;
            if self
                .progress
                .is_some_and(|(c, _)| !self.records.get(c).is_some_and(|r| r.explicit))
            {
                self.progress = None;
            }
            self.lighting = Some(lighting);
        }
        self.cooldown = self.cooldown.saturating_sub(1);
        if !self.pending.contains(&true) {
            self.waited = 0;
            return Ok(Vec::new());
        }
        if !settled.gi {
            // Faces lit before and after a change of probe light would not match.
            self.progress = None;
            return Ok(Vec::new());
        }
        if !settled.textures && !sync && self.waited < TEXTURE_WAIT {
            self.waited += 1;
            return Ok(Vec::new());
        }
        Ok(self.schedule(sync))
    }

    /// The due faces (and finishes) from the current progress.
    fn schedule(&mut self, sync: bool) -> Vec<Work> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let due: Vec<usize> = (0..self.records.len())
            .filter(|&i| self.pending[i])
            .collect();
        let mut cursor = self
            .progress
            .filter(|(c, _)| self.pending.get(*c).copied().unwrap_or(false))
            .or_else(|| due.first().map(|&c| (c, 0)));
        // Steps 0..FACES render faces; step FACES reduces and prefilters, in a live
        // frame of its own.
        let mut work = Vec::new();
        while let Some((capture, step)) = cursor {
            let resolution = layout.columns[capture].1;
            if step < FACES {
                work.push(Work::Face {
                    capture,
                    face: step,
                    position: self.records[capture].capture_point(),
                    resolution,
                });
                cursor = Some((capture, step + 1));
            } else {
                work.push(Work::Finish {
                    capture,
                    resolution,
                });
                cursor = due.iter().copied().find(|&c| c > capture).map(|c| (c, 0));
            }
            if !sync {
                break;
            }
        }
        self.progress = if sync { None } else { cursor };
        work
    }

    /// A capture's prefilter landed: shading reads it from the next relight on.
    pub(crate) fn finished(&mut self, capture: usize) {
        if let Some(pending) = self.pending.get_mut(capture) {
            *pending = false;
            self.ready[capture] = true;
            self.bakes[capture] += 1;
            self.rebaseline |= 1 << capture;
            if self.sync {
                self.instant |= 1 << capture;
            }
        }
        if !self.pending.contains(&true) {
            // A round done: the next bounce bakes every automatic capture again,
            // now shading with this round.
            if self.bounces > 0 && self.records.iter().any(|r| !r.explicit) {
                self.bounces -= 1;
                for (pending, record) in self.pending.iter_mut().zip(&self.records) {
                    *pending = !record.explicit;
                }
                return;
            }
            self.cooldown = COOLDOWN;
            self.waited = 0;
        }
    }

    /// The record of explicit probe `key`, if the quality holds it.
    pub(crate) fn index(&self, key: u64) -> Option<usize> {
        self.records.iter().position(|r| r.explicit && r.key == key)
    }

    /// Requests a bake of explicit probe `key` (no cooldown); false when the renderer
    /// holds no such record (unknown, or past the quality's capture count).
    pub(crate) fn request(&mut self, key: u64) -> bool {
        let Some(i) = self.index(key) else {
            return false;
        };
        self.pending[i] = true;
        true
    }

    /// Drops a requested, unpublished bake of `key` (its column keeps its image); false
    /// when nothing was pending. A bake publishes within one frame's work, so between
    /// frames every pending bake can still be cancelled.
    pub(crate) fn cancel(&mut self, key: u64) -> bool {
        let Some(i) = self.index(key) else {
            return false;
        };
        let was = std::mem::replace(&mut self.pending[i], false);
        if self.progress.is_some_and(|(c, _)| c == i) {
            self.progress = None;
        }
        was
    }

    /// Explicit probe `key`'s bake state, if the quality holds its record.
    pub(crate) fn status(&self, key: u64) -> Option<BakeStatus> {
        let i = self.index(key)?;
        Some(BakeStatus {
            pending: self.pending[i],
            step: self.progress.filter(|(c, _)| *c == i).map(|(_, step)| step),
            steps: FACES + 1,
            ready: self.ready[i],
            bakes: self.bakes[i],
            resolution: self.layout.as_ref()?.columns.get(i)?.1,
        })
    }

    /// Takes the restore flag set by a new atlas, a moved column or a loaded image. A
    /// restore rewrites the whole atlas, scratch column included, so a bake under way
    /// starts its faces again.
    pub(crate) fn take_restore(&mut self) -> bool {
        let restore = std::mem::take(&mut self.restore) && self.gpu.is_some();
        if restore {
            self.progress = None;
        }
        restore
    }

    /// Asks for a restore of the explicit columns (an image was loaded).
    pub(crate) fn request_restore(&mut self) {
        self.restore = true;
    }

    /// Whether record `capture` is explicit.
    pub(crate) fn is_explicit(&self, capture: usize) -> bool {
        self.records.get(capture).is_some_and(|r| r.explicit)
    }

    /// Record `capture`'s key and bakes published so far.
    pub(crate) fn key_and_bakes(&self, capture: usize) -> Option<(u64, u64)> {
        Some((self.records.get(capture)?.key, *self.bakes.get(capture)?))
    }

    /// The explicit records and their columns (`key`, x, resolution) for a restore.
    pub(crate) fn explicit_columns(&self) -> Vec<(usize, u64, u32, u32)> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        self.records
            .iter()
            .zip(&layout.columns)
            .enumerate()
            .filter(|(_, (r, _))| r.explicit)
            .map(|(i, (r, &(x, n)))| (i, r.key, x, n))
            .collect()
    }

    /// Marks record `capture` as holding a restored image shading may read.
    pub(crate) fn restored(&mut self, capture: usize) {
        if let Some(ready) = self.ready.get_mut(capture) {
            *ready = true;
            self.rebaseline |= 1 << capture;
            self.instant |= 1 << capture;
        }
    }

    /// Whether an automatic capture still waits for its bake: specs not yet planned,
    /// or a record pending or not yet ready (none while failed).
    pub(crate) fn awaiting_bake(&self) -> bool {
        !self.failed
            && self.specs.iter().any(|s| !s.explicit)
            && (self.unplanned
                || self
                    .records
                    .iter()
                    .zip(self.pending.iter().zip(&self.ready))
                    .any(|(record, (pending, ready))| !record.explicit && (*pending || !ready)))
    }

    /// The atlas extent (texels).
    pub(crate) fn atlas_size(&self) -> Option<[u32; 2]> {
        Some(self.layout.as_ref()?.size)
    }

    fn upload_records(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        let (Some(layout), Some(state)) = (&self.layout, &self.gpu) else {
            return Ok(());
        };
        let mut bytes = vec![0u8; MAX_CAPTURES * RECORD_BYTES];
        for (i, (record, &(x, n))) in self.records.iter().zip(&layout.columns).enumerate() {
            bytes[i * RECORD_BYTES..(i + 1) * RECORD_BYTES]
                .copy_from_slice(&record.record([x, 0], n));
        }
        gpu.upload_queued(state.records, &bytes)?;
        self.copy = true;
        Ok(())
    }

    /// Whether shading reads captures this frame (`enabled`: the quality has them).
    fn active(&self, enabled: bool) -> bool {
        // Bakes shade with the ready captures too (their previous round), so captures
        // reflect each other ([`BOUNCES`]); a bake writes only the scratch column until
        // its prefilter, which no shading overlaps.
        enabled && !self.failed && self.gpu.is_some() && self.ready.contains(&true)
    }

    /// Writes this frame's Look rows (zero rows: no captures): shading visits only
    /// the baked captures whose influence box meets the camera's frustum
    /// (`view_projection`).
    pub(crate) fn write(&self, frame: &mut LookFrame, enabled: bool, view_projection: Mat4) {
        let (Some(gpu), Some(layout), true) = (&self.gpu, &self.layout, self.active(enabled))
        else {
            return;
        };
        let visible = self
            .records
            .iter()
            .zip(&self.ready)
            .enumerate()
            .filter(|(_, (record, ready))| **ready && record.in_view(view_projection))
            .fold(0u32, |mask, (i, _)| mask | 1 << i);
        frame.capture_info = [
            [
                gpu.records_index,
                self.records.len() as u32,
                gpu.sampled,
                gpu.frame_index,
            ],
            [layout.size[0], layout.size[1], visible, debug_modes()],
        ];
    }

    /// The relight's root word 2: records whose bake landed since the last relight
    /// (low half; their baseline is measured) and whose baseline it clears (high half).
    pub(crate) fn refit_masks(&self) -> u32 {
        (self.rebaseline & 0xffff) | (self.reset & 0xffff) << 16
    }

    /// The relight's root word 4: records whose new bake shows at once (bit per record).
    pub(crate) fn instant_mask(&self) -> u32 {
        self.instant
    }

    /// Whether this frame's relight first copies the records into the frame buffer
    /// (they changed since its last copy; the copies persist).
    pub(crate) fn copy_records(&self) -> bool {
        self.copy
    }

    /// Imports what shading reads, when it reads captures this frame.
    pub(crate) fn import(&self, graph: &mut Graph, enabled: bool) -> Option<CaptureIds> {
        let gpu = self.gpu.as_ref().filter(|_| self.active(enabled))?;
        let state = |access| State {
            access,
            queue: Queue::Graphics,
        };
        Some(CaptureIds {
            atlas: graph.import(Resource::Image(gpu.atlas), gpu.atlas_state),
            records: graph.import(Resource::Buffer(gpu.records), state(Access::HostWrite)),
            frame: graph.import(Resource::Buffer(gpu.frame), state(gpu.frame_access)),
        })
    }

    /// Records the final states of a frame that read the captures; its relight
    /// took every pending baseline.
    pub(crate) fn finish(&mut self, ids: &CaptureIds, final_states: &[State]) {
        if let Some(gpu) = &mut self.gpu {
            gpu.atlas_state = final_states[ids.atlas.0];
            gpu.frame_access = final_states[ids.frame.0].access;
            self.rebaseline = 0;
            self.reset = 0;
            self.instant = 0;
            self.copy = false;
        }
    }

    /// The atlas a bake writes: image, storage index, state and the scratch
    /// column's x.
    pub(crate) fn atlas(&self) -> Option<(Image, u32, State, u32)> {
        let gpu = self.gpu.as_ref()?;
        Some((
            gpu.atlas,
            gpu.storage,
            gpu.atlas_state,
            self.layout.as_ref()?.scratch,
        ))
    }

    /// A bake graph left the atlas in `state`.
    pub(crate) fn set_atlas_state(&mut self, state: State) {
        if let Some(gpu) = &mut self.gpu {
            gpu.atlas_state = state;
        }
    }

    /// Atlas column x of record `capture`.
    pub(crate) fn column(&self, capture: usize) -> Option<u32> {
        Some(self.layout.as_ref()?.columns.get(capture)?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slab(lo: [f32; 3], hi: [f32; 3], smooth: bool) -> ([Vec3; 2], bool) {
        ([Vec3::from_array(lo), Vec3::from_array(hi)], smooth)
    }

    #[test]
    fn levels_stack_bordered_halving_tiles() {
        assert_eq!(level_y(256, 0), 0);
        assert_eq!(level_y(256, 1), 258);
        assert_eq!(level_y(256, 2), 258 + 130);
        assert_eq!(level_y(256, 4), 258 + 130 + 66 + 34);
        assert_eq!(level_y(256, LEVELS), 258 + 130 + 66 + 34 + 18);
        let layout = Layout::new(&[256, 128]);
        assert_eq!(layout.columns, vec![(0, 256), (258, 128)]);
        assert_eq!(layout.scratch, 388);
        assert_eq!(layout.size, [388 + 258, level_y(256, LEVELS)]);
    }

    #[test]
    fn records_map_world_into_the_rotated_frame() {
        let spec = CaptureSpec {
            position: Vec3::new(1., 2., 3.),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            influence: Vec3::new(4., 2., 1.),
            blend: 0.5,
            projection_centre: Vec3::ZERO,
            projection: Vec3::new(4., 2., 1.),
            capture_offset: Vec3::new(0., 1., 0.),
            priority: 0,
            resolution: 0,
            intensity: 1.,
            key: 0,
            explicit: false,
        };
        let bytes = spec.record([10, 0], 128);
        let words = bytes.as_chunks::<4>().0;
        let floats: Vec<f32> = words.iter().map(|w| f32::from_le_bytes(*w)).collect();
        // Local x of the entity points along world -z: world (1, 2, 2) is local x = 1.
        let world = Vec3::new(1., 2., 2.);
        let local: Vec<f32> = (0..3)
            .map(|i| Vec3::from_slice(&floats[i * 4..i * 4 + 3]).dot(world) + floats[i * 4 + 3])
            .collect();
        assert!((local[0] - 1.).abs() < 1e-5 && local[1].abs() < 1e-5 && local[2].abs() < 1e-5);
        assert_eq!(&floats[24..27], &[1., 3., 3.]);
        let tile: Vec<u32> = words[28..].iter().map(|w| u32::from_le_bytes(*w)).collect();
        assert_eq!(tile, [10, 0, 128, 0]);
    }

    /// A stage like the Showcase: a wide floor, a figure at its centre and a grid of
    /// smooth spheres off to one side.
    fn stage() -> Vec<([Vec3; 2], bool)> {
        let mut boxes = vec![
            slab([-15., -0.2, -15.], [15., 0.12, 15.], false),
            slab([-0.97, 0.12, -0.16], [0.97, 1.95, 0.21], false),
        ];
        for row in 0..3 {
            for column in 0..6 {
                let (x, z) = (-2.25 + column as f32 * 0.9, 3. + row as f32 * 0.9);
                boxes.push(slab([x - 0.3, 0., z - 0.3], [x + 0.3, 0.6, z + 0.3], true));
            }
        }
        boxes
    }

    #[test]
    fn the_stage_is_captured_from_open_space_near_its_smooth_objects() {
        let boxes = stage();
        let specs = place(&boxes, 8);
        assert_eq!(specs.len(), 1, "{specs:?}");
        let spec = specs[0];
        spec.validate().expect("valid automatic capture");
        // Nothing within the clearance, eye level or above, near the spheres.
        let point = spec.capture_point();
        for (b, _) in &boxes[1..] {
            assert!(outside(b, point) >= OPEN - 1e-3, "{point} near {b:?}");
        }
        assert!(point.y >= 0.12 + 1.2 - 1e-3, "{point}");
        assert!(point.x.abs() < 2. && (point.z - 3.9).abs() < 2., "{point}");
        // Projected onto the floor's top.
        let floor = spec.position.y + spec.projection_centre.y - spec.projection.y;
        assert!((floor - 0.12).abs() < 1e-4, "{floor}");
    }

    #[test]
    fn far_smooth_clusters_get_their_own_open_capture() {
        let mut boxes = stage();
        boxes[0] = slab([-40., -0.2, -40.], [40., 0.12, 40.], false);
        boxes.push(slab([29., 0., 29.], [31., 1., 31.], true));
        let specs = place(&boxes, 8);
        assert_eq!(specs.len(), 2, "{specs:?}");
        let far = specs[1];
        assert_eq!(far.priority, 1);
        let point = far.capture_point();
        assert!(
            (point - Vec3::new(30., point.y, 30.)).length() < 4.,
            "{point}"
        );
        for (b, _) in &boxes[1..] {
            assert!(outside(b, point) >= OPEN - 1e-3, "{point} near {b:?}");
        }
    }

    #[test]
    fn huge_worlds_only_capture_their_smooth_clusters() {
        let boxes = [
            slab([-2000., -10., -2000.], [2000., 0., 2000.], false),
            slab([4., 0., 4.], [5., 1., 5.], true),
        ];
        let specs = place(&boxes, 4);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].priority, 1);
        assert!(place(&[], 4).is_empty());
    }

    #[test]
    fn shading_visits_only_captures_in_the_frustum() {
        let view = Mat4::perspective_infinite_reverse_rh(1., 1., 0.1)
            * Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let at = |z: f32| CaptureSpec {
            position: Vec3::new(0., 0., z),
            ..place(&[slab([-1., 0., -1.], [1., 1., 1.], true)], 1)[0]
        };
        assert!(at(-10.).in_view(view));
        assert!(!at(10.).in_view(view));
        // A box around the eye always meets the frustum.
        assert!(at(0.).in_view(view));
    }

    #[test]
    fn lighting_changes_need_a_turn_or_a_new_look() {
        let base = Lighting {
            content: "a".into(),
            sun: [0., 1., 0., 1000., 1000., 1000.],
            look: (None, None),
            lights: 1,
            gi: 0,
        };
        let mut dimmer = base.clone();
        dimmer.sun[3..].copy_from_slice(&[990.; 3]);
        assert!(!base.differs(&dimmer));
        let mut turned = base.clone();
        turned.sun[..3].copy_from_slice(&[0.1, 1., 0.]);
        assert!(base.differs(&turned));
        let mut gi = base.clone();
        gi.gi = 4096;
        assert!(base.differs(&gi));
    }

    #[test]
    fn live_bakes_take_one_face_per_frame_and_sync_bakes_take_all() {
        let mut captures = Captures {
            records: vec![place(&[slab([-1., 0., -1.], [1., 1., 1.], true)], 1)[0]; 2],
            layout: Some(Layout::new(&[64, 64])),
            pending: vec![true, true],
            ready: vec![false, false],
            bakes: vec![0, 0],
            ..Default::default()
        };
        let first = captures.schedule(false);
        assert!(matches!(
            first[..],
            [Work::Face {
                capture: 0,
                face: 0,
                ..
            }]
        ));
        for face in 1..FACES {
            let work = captures.schedule(false);
            assert!(matches!(work[..], [Work::Face { capture: 0, face: f, .. }] if f == face));
        }
        let finish = captures.schedule(false);
        assert!(matches!(finish[..], [Work::Finish { capture: 0, .. }]));
        assert_eq!(captures.progress, Some((1, 0)));
        captures.finished(0);
        assert_eq!(captures.refit_masks() & 0xffff, 1);
        let all = captures.schedule(true);
        assert_eq!(all.len(), FACES as usize + 1);
        assert!(matches!(all.last(), Some(Work::Finish { capture: 1, .. })));
    }
}
