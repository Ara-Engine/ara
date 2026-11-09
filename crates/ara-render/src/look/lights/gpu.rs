//! The V2 passes on the GPU (`shaders/lights-v2.slang`): their pipelines, the per-frame
//! storage they share and the order [`LightsGpu::record`] submits them in.
//!
//! Inputs are the renderer's: the raster frame record (root word 0) names the
//! G-buffer, the 112-byte light records (`Frame.lighting.w`) and the look record whose
//! LTC tables `localLight` reads; the trace scene record comes from
//! `look::trace::TraceGpu::record`. The output is one storage image of local-light
//! radiance (linear, not pre-exposed) that a lighting pass adds.
use anyhow::{Result, ensure};
use ara_gpu::commands::{Command, Recorder};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Pipeline, Resource, RootConstants, vk,
};
use bytemuck::{Pod, Zeroable};
use std::path::Path;

/// Pixels per fine tile side (`LIGHTS_TILE`).
pub(crate) const TILE: u32 = 16;
/// Fine tiles per coarse tile side (`LIGHTS_COARSE`).
pub(crate) const COARSE: u32 = 4;
/// `LightsRecord.trace.w` flags (`LIGHTS_OMIT`, `LIGHTS_STATS`, `LIGHTS_UNSHADOWED`).
pub(crate) const FLAG_OMIT: u32 = 1;
pub(crate) const FLAG_STATS: u32 = 2;
pub(crate) const FLAG_UNSHADOWED: u32 = 4;
pub(crate) const FLAG_NOCLASSIFY: u32 = 8;
pub(crate) const FLAG_NOMASK: u32 = 16;
pub(crate) const FLAG_DEBUG: u32 = 32;
/// `TRACE_NONE`: no trace scene (every pair unshadowed).
pub(crate) const NO_TRACE: u32 = u32::MAX;
/// Stats words the passes count into (`LIGHTS_STAT_*`).
const STAT_WORDS: usize = 8;
/// Bytes per light bound, tile bound, tile head and pool entry.
const LIGHT_BYTES: u64 = 32;
const TILE_BYTES: u64 = 80;
const HEAD_BYTES: u64 = 8;
const ENTRY_BYTES: u64 = 16;

/// The pass record (`lights-v2.slangh` `LightsRecord`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Record {
    counts: [u32; 4],
    buffers: [u32; 4],
    output: [u32; 4],
    trace: [u32; 4],
    params: [f32; 4],
    extra: [u32; 4],
}

/// Storage sizes, fixed at creation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Capacity {
    /// Render size in pixels.
    pub size: [u32; 2],
    /// Most lights a frame may hold.
    pub lights: u32,
    /// Lights one 64x64 coarse tile lists before it falls back to scanning all.
    pub coarse_lights: u32,
    /// Tile entries a frame may allocate (all tiles together) before tiles fall back.
    pub entries: u32,
}

/// One frame's inputs beyond the frame record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Inputs {
    /// Lights in the frame record's light buffer.
    pub lights: u32,
    /// Storage-image index of the radiance output (RGBA, rgb radiance).
    pub output: u32,
    /// Storage-image index of the per-pixel counters (lit lights, rays, listed,
    /// omitted); `None` skips them (timing runs).
    pub stats: Option<u32>,
    /// Trace scene record; `None` shades unshadowed.
    pub trace: Option<u32>,
    /// Instance mask visibility sees.
    pub mask: u8,
    /// Shading and overload settings.
    pub settings: super::Settings,
    /// Pixel footprint per metre of view distance (2 tan(fov / 2) / height).
    pub footprint: f32,
}

/// Counters a frame's passes reported (`LIGHTS_STAT_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    /// Tiles whose entries did not fit the pool (shaded through the fallback).
    pub pool_overflow: u32,
    /// Coarse tiles whose lists overflowed (their tiles scanned every light).
    pub coarse_overflow: u32,
    /// Tiles with more candidates than the select stage holds.
    pub stage_overflow: u32,
    /// Entries allocated.
    pub entries: u32,
    /// Partial pairs (traced per pixel).
    pub partial: u32,
    /// Partial pairs that trace the whole TLAS (more than two instances).
    pub all: u32,
    /// Classification node and triangle tests.
    pub classification_tests: u32,
}

impl Stats {
    /// Whether every tile shaded from its own list (no fallback).
    pub(crate) fn clean(&self) -> bool {
        self.pool_overflow == 0 && self.coarse_overflow == 0 && self.stage_overflow == 0
    }
}

struct Pipelines {
    build: Pipeline,
    prep: Pipeline,
    bin: Pipeline,
    select: Pipeline,
    shade: Pipeline,
    /// The shade pass split by PSO: clear pairs without trace() code, then partial pairs.
    shade_lean: Pipeline,
    shade_vis: Pipeline,
    exact: Pipeline,
}

/// A storage buffer and its heap index.
#[derive(Clone, Copy)]
struct Slot {
    buffer: Buffer,
    index: u32,
}

/// The V2 passes and their storage for one render size.
pub(crate) struct LightsGpu {
    pipelines: Pipelines,
    capacity: Capacity,
    tiles: [u32; 2],
    coarse: [u32; 2],
    record: Slot,
    bounds: Slot,
    lists: Slot,
    tile_bounds: Slot,
    heads: Slot,
    pool: Slot,
    stats: Slot,
    readback: Buffer,
    /// Whether the storage has been used (barrier sources).
    used: bool,
    /// This frame shades in one combined pass (omission orders all entries together;
    /// per-pixel counters cover all of them).
    combined: bool,
}

/// Loads cooked pipeline `name` from `artifacts`.
pub(crate) fn load(gpu: &mut GpuDevice, artifacts: &Path, name: &str) -> Result<Pipeline> {
    let cooked: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
    Ok(gpu.create_pipeline(&cooked, artifacts)?)
}

/// The renderer's cooked artifact directory.
pub(crate) fn artifacts() -> std::path::PathBuf {
    ara_gpu::cooked::resource_dir(
        env!("CARGO_PKG_NAME"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
    )
}

impl LightsGpu {
    /// Loads the passes and allocates storage for `capacity`.
    pub(crate) fn new(gpu: &mut GpuDevice, capacity: Capacity) -> Result<Self> {
        ensure!(
            capacity.size.iter().all(|&s| s > 0) && capacity.lights > 0,
            "Empty V2 light capacity"
        );
        let artifacts = artifacts();
        let pipelines = Pipelines {
            build: load(gpu, &artifacts, "lights_build")?,
            prep: load(gpu, &artifacts, "lights_prep")?,
            bin: load(gpu, &artifacts, "lights_bin")?,
            select: load(gpu, &artifacts, "lights_select")?,
            shade: load(gpu, &artifacts, "lights_shade")?,
            shade_lean: load(gpu, &artifacts, "lights_shade_lean")?,
            shade_vis: load(gpu, &artifacts, "lights_shade_vis")?,
            exact: load(gpu, &artifacts, "lights_exact")?,
        };
        let tiles = capacity.size.map(|s| s.div_ceil(TILE));
        let coarse = tiles.map(|t| t.div_ceil(COARSE));
        let tile_count = u64::from(tiles[0]) * u64::from(tiles[1]);
        let coarse_count = u64::from(coarse[0]) * u64::from(coarse[1]);
        let mut slot = |bytes: u64, host: bool| -> Result<Slot> {
            let usage = vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST;
            let buffer = gpu.create_buffer(bytes.max(16), usage, host)?;
            let index = gpu.storage_buffer(buffer)?.index;
            Ok(Slot { buffer, index })
        };
        let record = slot(std::mem::size_of::<Record>() as u64, true)?;
        let bounds = slot(u64::from(capacity.lights) * LIGHT_BYTES, false)?;
        let lists = slot(
            coarse_count * (u64::from(capacity.coarse_lights) + 1) * 4,
            false,
        )?;
        let tile_bounds = slot(tile_count * TILE_BYTES, false)?;
        let heads = slot(tile_count * HEAD_BYTES, false)?;
        let pool = slot(u64::from(capacity.entries) * ENTRY_BYTES, false)?;
        let stats = slot((STAT_WORDS * 4) as u64, false)?;
        let readback = gpu.create_buffer(
            (STAT_WORDS * 4) as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        Ok(Self {
            pipelines,
            capacity,
            tiles,
            coarse,
            record,
            bounds,
            lists,
            tile_bounds,
            heads,
            pool,
            stats,
            readback,
            used: false,
            combined: true,
        })
    }

    /// Bytes of V2's per-frame storage (light bounds, coarse lists, tile bounds and heads,
    /// the entry pool, stats and record).
    pub(crate) fn resident_bytes(&self) -> u64 {
        let tiles = u64::from(self.tiles[0]) * u64::from(self.tiles[1]);
        let coarse = u64::from(self.coarse[0]) * u64::from(self.coarse[1]);
        u64::from(self.capacity.lights) * LIGHT_BYTES
            + coarse * (u64::from(self.capacity.coarse_lights) + 1) * 4
            + tiles * (TILE_BYTES + HEAD_BYTES)
            + u64::from(self.capacity.entries) * ENTRY_BYTES
            + (STAT_WORDS * 4 + std::mem::size_of::<Record>()) as u64
    }

    /// Fine tiles per row and rows.
    pub(crate) fn tiles(&self) -> [u32; 2] {
        self.tiles
    }

    /// Writes this frame's record. The record is host-coherent storage: call it only
    /// once the previous frame's passes have completed.
    pub(crate) fn prepare(&mut self, gpu: &mut GpuDevice, inputs: &Inputs) -> Result<()> {
        ensure!(
            inputs.lights <= self.capacity.lights,
            "{} lights exceed the V2 capacity of {}",
            inputs.lights,
            self.capacity.lights
        );
        let s = inputs.settings;
        let mut flags = 0;
        if s.omission.is_some() {
            flags |= FLAG_OMIT;
        }
        if inputs.stats.is_some() {
            flags |= FLAG_STATS;
        }
        if !s.classify {
            flags |= FLAG_NOCLASSIFY;
        }
        if !s.masks {
            flags |= FLAG_NOMASK;
        }
        if s.debug_masks {
            flags |= FLAG_DEBUG;
        }
        if inputs.trace.is_none() {
            flags |= FLAG_UNSHADOWED;
        }
        let record = Record {
            counts: [inputs.lights, self.tiles[0], self.tiles[1], self.coarse[0]],
            buffers: [
                self.bounds.index,
                self.lists.index,
                self.tile_bounds.index,
                self.heads.index,
            ],
            output: [
                inputs.output,
                self.stats.index,
                self.capacity.coarse_lights,
                self.capacity.entries,
            ],
            trace: [
                inputs.trace.unwrap_or(NO_TRACE),
                s.samples.max(1),
                u32::from(inputs.mask),
                flags,
            ],
            params: [
                s.omission.unwrap_or(0.),
                s.bias_footprints,
                inputs.footprint,
                s.bias_min,
            ],
            extra: [
                self.pool.index,
                inputs.stats.unwrap_or(0),
                self.coarse[1],
                0,
            ],
        };
        gpu.upload(self.record.buffer, bytemuck::bytes_of(&record))?;
        self.combined = s.omission.is_some() || inputs.stats.is_some();
        Ok(())
    }

    fn storage(&self) -> [Buffer; 6] {
        [
            self.bounds.buffer,
            self.lists.buffer,
            self.tile_bounds.buffer,
            self.heads.buffer,
            self.pool.buffer,
            self.stats.buffer,
        ]
    }

    /// Dependencies between two passes over the shared storage.
    fn barrier(&self, c: &mut Recorder, first: bool) {
        c.barriers(self.storage().map(|buffer| Barrier {
            resource: Resource::Buffer(buffer),
            before: if first {
                Access::Undefined
            } else {
                Access::Storage
            },
            after: Access::Storage,
            handoff: None,
        }));
    }

    fn pass(
        &self,
        c: &mut Recorder,
        label: &str,
        pipeline: Pipeline,
        frame: u32,
        groups: [u32; 3],
    ) {
        c.push(Command::BeginPass(label.into()));
        c.push(Command::Pipeline(pipeline));
        c.push(Command::Constants(RootConstants {
            words: [frame, self.record.index, 0, 0, 0, 0, 0, 0],
        }));
        c.push(Command::Dispatch(groups));
        c.push(Command::EndPass);
    }

    /// Records the V2 passes (`Lights.Build`, `.Prep`, `.Bin`, `.Select`, then `.Shade`,
    /// or `.Shade` (clear pairs) and `.Visibility` (partial pairs, adding to the output)
    /// as two PSOs when [`Self::prepare`] needed no combined pass) with the frame record
    /// at heap index `frame`. The G-buffer must be readable and `output` (the image the
    /// inputs named) in `Storage`; [`Self::prepare`] wrote the record.
    pub(crate) fn record(&mut self, c: &mut Recorder, frame: u32, lights: u32, output: Image) {
        self.barrier(c, !self.used);
        self.used = true;
        c.push(Command::UpdateBuffer {
            destination: self.stats.buffer,
            offset: 0,
            bytes: vec![0; STAT_WORDS * 4],
        });
        let [tx, ty] = self.tiles;
        let [cx, cy] = self.coarse;
        self.pass(
            c,
            "Lights.Build",
            self.pipelines.build,
            frame,
            [lights.div_ceil(64).max(1), 1, 1],
        );
        self.pass(c, "Lights.Prep", self.pipelines.prep, frame, [tx, ty, 1]);
        self.barrier(c, false);
        self.pass(c, "Lights.Bin", self.pipelines.bin, frame, [cx, cy, 1]);
        self.barrier(c, false);
        self.pass(
            c,
            "Lights.Select",
            self.pipelines.select,
            frame,
            [tx, ty, 1],
        );
        self.barrier(c, false);
        if self.combined {
            self.pass(c, "Lights.Shade", self.pipelines.shade, frame, [tx, ty, 1]);
        } else {
            self.pass(
                c,
                "Lights.Shade",
                self.pipelines.shade_lean,
                frame,
                [tx, ty, 1],
            );
            c.barriers([Barrier {
                resource: Resource::Image(output),
                before: Access::Storage,
                after: Access::Storage,
                handoff: None,
            }]);
            self.pass(
                c,
                "Lights.Visibility",
                self.pipelines.shade_vis,
                frame,
                [tx, ty, 1],
            );
        }
        self.barrier(c, false);
    }

    /// Records the exact finite-range control (`Lights.Exact`): every light at every
    /// pixel, every source sample through the whole TLAS.
    pub(crate) fn record_exact(&mut self, c: &mut Recorder, frame: u32) {
        self.barrier(c, !self.used);
        self.used = true;
        let [w, h] = self.capacity.size;
        self.pass(
            c,
            "Lights.Exact",
            self.pipelines.exact,
            frame,
            [w.div_ceil(8), h.div_ceil(8), 1],
        );
        self.barrier(c, false);
    }

    /// Records a copy of the counters into the readback buffer; read them with
    /// [`Self::stats`] once the submission completed.
    pub(crate) fn record_stats_copy(&self, c: &mut Recorder) {
        c.barriers([Barrier {
            resource: Resource::Buffer(self.stats.buffer),
            before: Access::Storage,
            after: Access::CopySource,
            handoff: None,
        }]);
        c.push(Command::CopyBuffer {
            source: self.stats.buffer,
            destination: self.readback,
            size: (STAT_WORDS * 4) as u64,
        });
        c.barriers([
            Barrier {
                resource: Resource::Buffer(self.readback),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(self.stats.buffer),
                before: Access::CopySource,
                after: Access::Storage,
                handoff: None,
            },
        ]);
    }

    /// The counters of the last completed [`Self::record_stats_copy`].
    pub(crate) fn stats(&self, gpu: &GpuDevice) -> Result<Stats> {
        let bytes = gpu.read(self.readback)?;
        let w: &[u32] = bytemuck::cast_slice(&bytes[..STAT_WORDS * 4]);
        Ok(Stats {
            pool_overflow: w[0],
            coarse_overflow: w[1],
            stage_overflow: w[2],
            entries: w[3],
            partial: w[4],
            all: w[5],
            classification_tests: w[6],
        })
    }

    /// Retires every buffer (pipelines live with the device).
    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for buffer in self.storage() {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        gpu.retire(Resource::Buffer(self.record.buffer))?;
        gpu.retire(Resource::Buffer(self.readback))?;
        gpu.collect()?;
        Ok(())
    }
}
