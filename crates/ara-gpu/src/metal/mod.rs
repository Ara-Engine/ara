//! Metal 4 backend (D12, D15: Apple silicon, Metal 4 GPU family).
//!
//! [`MetalDevice`] implements the portable [`crate::Device`] surface and mirrors the
//! inherent [`crate::vulkan::VulkanDevice`] resource API (same ash-typed formats and
//! usage flags), with the same handle generations, two queue timelines and
//! completion-gated retirement:
//! - graphics and compute are separate `MTL4CommandQueue`s; each timeline is an
//!   `MTLSharedEvent` signalled after every submission and waited on for dependencies;
//! - shaders bind through Metal Shader Converter's explicit global root signature: a
//!   fresh top-level argument buffer per draw/dispatch ([`abi::TopLevel`]) bound
//!   through an `MTL4ArgumentTable`, pointing at converter descriptor tables;
//! - every buffer, texture, table and scratch chunk is in one residency set attached
//!   to both queues, since Metal 4 has no implicit residency for indirect resources.
//!
//! Also mirrored: deferred frame completion ([`Completion`]), off-thread resource and
//! pipeline preparation, and CAMetalLayer presentation ([`present::Presenter`]).
//!
//! Pass timings come from MTL4 timestamp heaps ([`timestamps`]) on one GPU clock.
//!
//! Pipeline binaries persist in one process-wide MTL4 archive ([`cache`]), saved when
//! the last device drops (never under shader validation).
//!
//! `ClearDepth` draws the cooked vertex-only `clear_depth` pipeline ([`clear`]).
//!
//! Shared surfaces (D19) use global IOSurfaces ([`external`]).
//!
//! Not yet available: acceleration structures, mesh/task stages,
//! shader hot reload and HDR output.
mod abi;
mod cache;
#[cfg(test)]
mod cache_tests;
mod clear;
mod completion;
mod encode;
pub mod external;
#[cfg(test)]
mod external_tests;
mod info;
mod native;
mod native_access;
#[cfg(test)]
mod parity_tests;
mod pipeline;
mod prepare;
pub mod present;
mod reload;
mod resources;
mod scratch;
mod serialized;
mod tables;
#[cfg(test)]
mod tests;
mod timestamps;
mod transfer;
mod worker;

pub use completion::Completion;

use crate::arena::Arena;
use crate::commands::{CommandList, PassTiming, Recorder};
use crate::*;
use ash::vk;
use native::{nil, ns_error};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use pipeline::NativePipeline;
use resources::{NativeBuffer, NativeImage};
use std::ptr::NonNull;

// MTLCreateSystemDefaultDevice returns nil in processes that don't link CoreGraphics
// (command-line tools, test binaries).
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

/// Timeline waits give up after this long: a faulted command buffer never signals.
const WAIT_TIMEOUT_MS: u64 = 20_000;

struct Owned<V> {
    raw: V,
    /// Last submission value per queue that may reference this resource.
    last: [u64; 2],
    heap: Option<HeapIndex>,
    /// Queue that ordered updates use, moved by explicit barrier handoffs.
    owner: Queue,
}

impl<V> Owned<V> {
    fn new(raw: V) -> Self {
        Self {
            raw,
            last: [0; 2],
            heap: None,
            owner: Queue::Graphics,
        }
    }
}

enum Retired {
    Buffer(Owned<NativeBuffer>),
    Image(Owned<NativeImage>),
    /// Held only to keep the pipeline state alive until completion.
    Pipeline {
        _native: NativePipeline,
        last: [u64; 2],
    },
}

/// Recording storage retained until its submission completes.
struct Pending {
    point: TimelinePoint,
    allocator: Retained<ProtocolObject<dyn MTL4CommandAllocator>>,
    chunks: Vec<scratch::Chunk>,
    /// Timestamps of a graph submission; presentation copies carry none.
    timing: Option<timestamps::Timing>,
}

/// A recorded command buffer waiting for its batch's native commit.
struct Batched {
    cmd: Retained<ProtocolObject<dyn MTL4CommandBuffer>>,
    point: TimelinePoint,
    /// Timeline values this list waits for, per queue.
    waits: [u64; 2],
}

/// Resolved pass timings kept for `resolve`/completion callers (bounded).
const RESOLVED_TIMINGS: usize = 1024;

/// Owned Metal 4 device, arenas, converter descriptor tables and queue timelines.
pub struct MetalDevice {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queues: [Retained<ProtocolObject<dyn MTL4CommandQueue>>; 2],
    events: [Retained<ProtocolObject<dyn MTLSharedEvent>>; 2],
    residency: Retained<ProtocolObject<dyn MTLResidencySet>>,
    residency_dirty: bool,
    compiler: cache::Compiler,
    table: Retained<ProtocolObject<dyn MTL4ArgumentTable>>,
    tables: tables::Tables,
    scratch: scratch::Pool,
    allocators: Vec<Retained<ProtocolObject<dyn MTL4CommandAllocator>>>,
    buffers: Arena<BufferKind, Owned<NativeBuffer>>,
    images: Arena<ImageKind, Owned<NativeImage>>,
    pipelines: Arena<PipelineKind, NativePipeline>,
    submitted: [u64; 2],
    /// Values committed to each queue; a deferred scope's batch is recorded
    /// (`submitted`) before it is committed at the scope's end or a queue change.
    flushed: [u64; 2],
    /// Recorded command buffers of one queue, committed together by `flush`.
    batch: Vec<Batched>,
    retired: Vec<Retired>,
    pending: Vec<Pending>,
    info: vulkan::DeviceInfo,
    /// Distinguishes this device's completions from another device's.
    identity: u64,
    /// Submissions of the active deferred-completion scope.
    deferred: Option<Vec<TimelinePoint>>,
    /// Submitted values when the scope began; every scoped submission waits on them.
    deferred_waits: [u64; 2],
    /// An ended scope awaits poll/wait.
    completion_pending: bool,
    resource_worker: Option<prepare::ResourceWorker>,
    pipeline_worker: Option<prepare::PipelineWorker>,
    timestamp_pool: timestamps::Pool,
    /// Pass timings of completed submissions, oldest first, until resolved.
    timings: std::collections::VecDeque<(TimelinePoint, Vec<PassTiming>)>,
    timing_window: crate::vulkan::timestamps::Window,
    waits: crate::vulkan::waits::Counter,
    /// The cooked `ClearDepth` pipeline, loaded on first use.
    clear: Option<clear::Clear>,
    /// Shader reload provenance (no hot reload on Metal yet, see `reload`).
    shader_status: vulkan::ShaderStatus,
    #[cfg(feature = "fidelity-lab")]
    pub(crate) lab_params: crate::tunables::Registry,
    submission_stats: vulkan::SubmissionStats,
    /// Last: released after the workers joined, so the archive flush races no compile.
    _cache: cache::Lease,
}

// SAFETY: Metal devices, queues, shared events, buffers, textures and pipeline states
// may be retained, released and used from any thread. The objects that are not
// thread-safe (command allocators, argument tables, residency sets, encoders) are only
// touched through `&mut self`. `&self` methods read capabilities, query or wait on
// shared events, update atomic wait counters and copy host-visible buffer contents,
// the same shared access `VulkanDevice` gives its mapped memory.
unsafe impl Send for MetalDevice {}
// SAFETY: see `Send` above; no `&self` method mutates non-atomic state.
unsafe impl Sync for MetalDevice {}

impl MetalDevice {
    /// Like [`crate::vulkan::VulkanDevice::new`], which adds the optional RT tier when
    /// available. Metal has no acceleration structures yet, so this is the raster
    /// device with `info().ray_tracing` false, and RT consumers take their fallback.
    pub fn new(
        window_extensions: &[std::ffi::CString],
        cache_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
        Self::new_raster(window_extensions, cache_dir)
    }
    /// Create the system default Metal 4 device. `_window_extensions` keeps the Vulkan
    /// constructor's signature for backend-neutral callers; Metal needs none.
    /// Raster-only like [`crate::vulkan::VulkanDevice::new_raster`]. `cache_dir` holds
    /// the persistent pipeline archive; the process's first device picks it ([`cache`]).
    pub fn new_raster(
        _window_extensions: &[std::ffi::CString],
        cache_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
        let cache_dir = cache_dir.into();
        native::pool(|| Self::create(&cache_dir))
    }
    fn create(cache_dir: &std::path::Path) -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().ok_or_else(|| nil("Metal device"))?;
        if !device.supportsFamily(MTLGPUFamily::Metal4) {
            return Err(GpuError::Contract(format!(
                "{} does not support the Metal 4 GPU family",
                device.name()
            )));
        }
        let queue = || {
            device
                .newMTL4CommandQueue()
                .ok_or_else(|| nil("MTL4 queue"))
        };
        let event = || device.newSharedEvent().ok_or_else(|| nil("shared event"));
        let queues = [queue()?, queue()?];
        let events = [event()?, event()?];
        let residency = device
            .newResidencySetWithDescriptor_error(&MTLResidencySetDescriptor::new())
            .map_err(|e| ns_error("residency set", &e))?;
        for queue in &queues {
            queue.addResidencySet(&residency);
        }
        let table_desc = MTL4ArgumentTableDescriptor::new();
        table_desc.setMaxBufferBindCount(abi::ARGUMENT_TABLE_BUFFERS);
        let table = device
            .newArgumentTableWithDescriptor_error(&table_desc)
            .map_err(|e| ns_error("argument table", &e))?;
        let tables = tables::Tables::new(&device)?;
        for buffer in tables.buffers() {
            residency.addAllocation(ProtocolObject::from_ref(buffer));
        }
        let info = info::device_info(&device);
        // Joined last: a failed creation above never counts as a live device.
        let (compiler, cache) = cache::acquire(&device, cache_dir)?;
        Ok(Self {
            device,
            queues,
            events,
            residency,
            residency_dirty: true,
            compiler,
            table,
            tables,
            scratch: scratch::Pool::default(),
            allocators: Vec::new(),
            buffers: Arena::default(),
            images: Arena::default(),
            pipelines: Arena::default(),
            submitted: [0; 2],
            flushed: [0; 2],
            batch: Vec::new(),
            retired: Vec::new(),
            pending: Vec::new(),
            info,
            identity: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            deferred: None,
            deferred_waits: [0; 2],
            completion_pending: false,
            resource_worker: None,
            pipeline_worker: None,
            timestamp_pool: timestamps::Pool::default(),
            timings: std::collections::VecDeque::new(),
            timing_window: Default::default(),
            waits: Default::default(),
            clear: None,
            shader_status: Default::default(),
            #[cfg(feature = "fidelity-lab")]
            lab_params: Default::default(),
            submission_stats: Default::default(),
            _cache: cache,
        })
    }

    /// Capabilities recorded at creation.
    pub fn info(&self) -> &vulkan::DeviceInfo {
        &self.info
    }
    /// Like [`crate::vulkan::VulkanDevice::split_loading`]: a device for the loading
    /// screen. On Metal it is a second `MetalDevice` over the same `MTLDevice`, as the
    /// presentation-only devices of the Hub and splash are, with its own queues, arenas
    /// and descriptor tables, so it never aliases this device's queues.
    /// # Safety
    /// The Vulkan contract (call before submitting work); Metal adds no requirement.
    pub unsafe fn split_loading(&mut self, cache: impl Into<std::path::PathBuf>) -> Result<Self> {
        if self.submitted != [0; 2] {
            return Err(GpuError::Contract(
                "Loading split requires an unused device".into(),
            ));
        }
        Self::new_raster(&[], cache)
    }
    /// Whether `other` submits graphics work on this device's graphics queue; Metal
    /// devices never share queues.
    pub fn shares_graphics_queue(&self, other: &Self) -> bool {
        std::ptr::eq(&*self.queues[0], &*other.queues[0])
    }
    /// The UI-attachment view of `image` ([`crate::NativeView`]): the handle itself,
    /// checked live like the Vulkan view lookup.
    pub fn image_view(&self, image: Image) -> Result<crate::NativeView> {
        self.images.get(image)?;
        Ok(image)
    }
    /// Wait until both queues finished every submission.
    pub fn wait_idle(&self) -> Result<()> {
        for queue in [Queue::Graphics, Queue::Compute] {
            let value = self.flushed[queue.index()];
            Device::wait(self, TimelinePoint { queue, value })?;
        }
        Ok(())
    }

    /// Reserved bindless slots, including resources awaiting retirement. Collect
    /// completed retirements first when checking a replacement for leaks.
    pub fn descriptor_usage(&self) -> crate::vulkan::DescriptorUsage {
        crate::vulkan::DescriptorUsage {
            sampled_images: self.tables.textures.live() as usize,
            storage_buffers: self.tables.buffers.live() as usize,
            storage_images: self.tables.storage_images.live() as usize,
            acceleration_structures: self.tables.accelerations.live() as usize,
        }
    }

    /// Allocate a buffer. Host buffers are shared (coherent) storage.
    pub fn create_buffer(
        &mut self,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: bool,
    ) -> Result<Buffer> {
        let raw = native::pool(|| resources::buffer(&self.device, size, usage, host))?;
        self.adopt_buffer(raw)
    }
    /// Make a created buffer resident and give it an arena identity.
    fn adopt_buffer(&mut self, raw: NativeBuffer) -> Result<Buffer> {
        self.residency
            .addAllocation(ProtocolObject::from_ref(&*raw.buffer));
        self.residency_dirty = true;
        self.buffers.insert(Owned::new(raw))
    }
    /// Allocate host-readable transfer memory (shared storage is CPU-cached here).
    pub fn create_readback_buffer(&mut self, size: u64) -> Result<Buffer> {
        self.create_buffer(size, vk::BufferUsageFlags::TRANSFER_DST, true)
    }
    /// Allocate a single-level image.
    pub fn create_image(
        &mut self,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image> {
        self.create_mip_image([width, height], format, 1, usage)
    }
    /// Allocate a texture with native mip levels, including BC formats.
    pub fn create_mip_image(
        &mut self,
        size: [u32; 2],
        format: vk::Format,
        mip_levels: u32,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image> {
        let raw = native::pool(|| resources::image(&self.device, size, format, mip_levels, usage))?;
        self.adopt_image(raw)
    }
    /// Make a created texture resident and give it an arena identity.
    fn adopt_image(&mut self, raw: NativeImage) -> Result<Image> {
        self.residency
            .addAllocation(ProtocolObject::from_ref(&*raw.texture));
        self.residency_dirty = true;
        self.images.insert(Owned::new(raw))
    }
    /// Write a host buffer after every preceding use has completed.
    #[track_caller]
    pub fn upload(&self, buffer: Buffer, bytes: &[u8]) -> Result<()> {
        self.upload_range(buffer, 0, bytes)
    }
    /// Write a byte range of a host buffer after every preceding use has completed.
    #[track_caller]
    pub fn upload_range(&self, buffer: Buffer, offset: u64, bytes: &[u8]) -> Result<()> {
        let b = self.buffers.get(buffer)?;
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < b.last[q]) {
            return Err(GpuError::Contract(format!(
                "Upload would overwrite in-flight storage at {} ({buffer:?})",
                std::panic::Location::caller()
            )));
        }
        resources::write(&b.raw, offset, bytes)
    }
    /// Read host storage written by work up to `point`, once it completed. Never waits.
    pub fn read_completed(&self, buffer: Buffer, point: TimelinePoint) -> Result<Vec<u8>> {
        let b = self.buffers.get(buffer)?;
        if self.completed()?[point.queue.index()] < point.value {
            return Err(GpuError::Contract("Readback still in flight".into()));
        }
        resources::read(&b.raw)
    }
    /// Read host storage after every referencing submission completed.
    pub fn read(&self, buffer: Buffer) -> Result<Vec<u8>> {
        let b = self.buffers.get(buffer)?;
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < b.last[q]) {
            return Err(GpuError::Contract("Readback still in flight".into()));
        }
        resources::read(&b.raw)
    }
    /// Publish a sampled image into a fresh immutable texture-table slot.
    pub fn sampled_image(&mut self, image: Image, _depth: bool) -> Result<HeapIndex> {
        let value = self.images.get_mut(image)?;
        if !value.raw.usage.contains(vk::ImageUsageFlags::SAMPLED) {
            return Err(GpuError::Contract(
                "Image was not allocated for sampling".into(),
            ));
        }
        if let Some(index) = value.heap {
            return Ok(index);
        }
        let entry = abi::TableEntry::texture(value.raw.texture.gpuResourceID().to_raw());
        let index = self.tables.textures.insert(entry)?;
        value.heap = Some(index);
        Ok(index)
    }
    /// Publish a storage buffer into a fresh immutable buffer-table slot.
    pub fn storage_buffer(&mut self, buffer: Buffer) -> Result<HeapIndex> {
        let value = self.buffers.get_mut(buffer)?;
        if !value
            .raw
            .usage
            .contains(vk::BufferUsageFlags::STORAGE_BUFFER)
        {
            return Err(GpuError::Contract(
                "Buffer was not allocated for shader storage".into(),
            ));
        }
        if let Some(index) = value.heap {
            return Ok(index);
        }
        let entry = abi::TableEntry::buffer(value.raw.buffer.gpuAddress(), value.raw.size);
        let index = self.tables.buffers.insert(entry)?;
        value.heap = Some(index);
        Ok(index)
    }
    /// Publish a storage-image view. Slot lifetime follows image retirement.
    pub fn storage_image(&mut self, image: Image) -> Result<HeapIndex> {
        let value = self.images.get_mut(image)?;
        if !value.raw.usage.contains(vk::ImageUsageFlags::STORAGE) {
            return Err(GpuError::Contract(
                "Image was not allocated for shader storage".into(),
            ));
        }
        if let Some(index) = value.raw.storage {
            return Ok(index);
        }
        let entry = abi::TableEntry::texture(value.raw.texture.gpuResourceID().to_raw());
        let index = self.tables.storage_images.insert(entry)?;
        value.raw.storage = Some(index);
        Ok(index)
    }
    /// Create a pipeline exclusively from a hashed offline cook with Metal artifacts.
    pub fn create_pipeline(
        &mut self,
        cooked: &cooked::CookedPipeline,
        artifacts: &std::path::Path,
    ) -> Result<Pipeline> {
        let created =
            native::pool(|| pipeline::create(&self.device, &self.compiler, cooked, artifacts))?;
        self.pipelines.insert(created)
    }
    /// Invalidate a pipeline identity; release it after all queues complete.
    pub fn retire_pipeline(&mut self, pipeline: Pipeline) -> Result<()> {
        let native = self.pipelines.remove(pipeline)?;
        self.retired.push(Retired::Pipeline {
            _native: native,
            last: self.submitted,
        });
        Ok(())
    }
    /// Start an owned CPU recorder. Submission validates all resource handles.
    pub fn begin(&self, queue: Queue) -> Recorder {
        Recorder::new(queue)
    }
    /// Native mip-zero extent.
    pub fn image_extent(&self, image: Image) -> Result<[u32; 2]> {
        Ok(self.images.get(image)?.raw.size)
    }
    /// Metal API/shader validation failures abort the process under
    /// `MTL_DEBUG_LAYER=1`; a live device has no deferred diagnostics to report.
    pub fn assert_clean(&self) -> Result<()> {
        Ok(())
    }
    /// Always empty, for the same reason as [`Self::assert_clean`].
    pub fn diagnostics(&self) -> Vec<vulkan::Diagnostic> {
        Vec::new()
    }
    /// Cumulative queue submissions; on Metal every command list is one commit, and
    /// presentation copies are excluded like Vulkan's native integrations.
    pub fn submission_stats(&self) -> vulkan::SubmissionStats {
        self.submission_stats
    }

    /// Submit one list with timeline dependencies. Returns a monotonic signal point.
    /// Bindless reachability is conservative: all published resources are retained.
    pub fn submit(&mut self, list: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        native::pool(|| self.submit_now(list, waits))
    }
    fn submit_now(&mut self, list: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        // Inside a deferred scope, every submission also waits on the work submitted
        // before the scope began (the boundary a synchronous resolve used to give).
        let mut merged = Vec::new();
        let waits = if self.deferred.is_some() {
            let mut dependencies = self.deferred_waits;
            for point in waits {
                let value = &mut dependencies[point.queue.index()];
                *value = (*value).max(point.value);
            }
            for queue in [Queue::Graphics, Queue::Compute] {
                let value = dependencies[queue.index()];
                if value > 0 {
                    merged.push(TimelinePoint { queue, value });
                }
            }
            &merged[..]
        } else {
            waits
        };
        for wait in waits {
            if wait.value > self.submitted[wait.queue.index()] {
                return Err(GpuError::Contract("Wait targets unsubmitted work".into()));
            }
        }
        self.reclaim()?;
        let queue = list.queue;
        if self.batch.first().is_some_and(|b| b.point.queue != queue) {
            self.flush();
        }
        let point = TimelinePoint {
            queue,
            value: self.submitted[queue.index()]
                .checked_add(1)
                .ok_or_else(|| GpuError::Contract("Queue timeline exhausted".into()))?,
        };
        let allocator = match self.allocators.pop() {
            Some(allocator) => allocator,
            None => self
                .device
                .newCommandAllocator()
                .ok_or_else(|| nil("command allocator"))?,
        };
        let cmd = self
            .device
            .newCommandBuffer()
            .ok_or_else(|| nil("command buffer"))?;
        cmd.beginCommandBufferWithAllocator(&allocator);
        let mut scratch = scratch::Scratch::default();
        let mut created = Vec::new();
        let labels = list
            .commands
            .iter()
            .filter_map(|command| match command {
                commands::Command::BeginPass(label) => Some(label.clone()),
                _ => None,
            })
            .collect();
        let mut timing = self.timestamp_pool.take(&self.device, labels);
        let heap = timing.as_ref().map(|timing| (&*timing.heap, timing.base));
        let encoded = self.encode(&cmd, &list, &mut scratch, &mut created, heap);
        cmd.endCommandBuffer();
        for chunk in &created {
            self.residency
                .addAllocation(ProtocolObject::from_ref(&**chunk));
            self.residency_dirty = true;
        }
        let aliases = match encoded {
            Ok(aliases) => aliases,
            Err(error) => {
                // Nothing was committed: the allocator and scratch are immediately
                // reusable.
                allocator.reset();
                self.allocators.push(allocator);
                self.scratch.recycle(scratch.chunks);
                if let Some(timing) = &timing {
                    self.timestamp_pool.recycle(timing);
                }
                return Err(error);
            }
        };
        if let Some(timing) = &mut timing {
            timing.aliases = aliases;
        }
        let mut dependencies = [0; 2];
        for wait in waits {
            let value = &mut dependencies[wait.queue.index()];
            *value = (*value).max(wait.value);
        }
        self.batch.push(Batched {
            cmd,
            point,
            waits: dependencies,
        });
        self.submitted[queue.index()] = point.value;
        for value in self.buffers.values_mut() {
            value.last[queue.index()] = point.value;
        }
        for value in self.images.values_mut() {
            value.last[queue.index()] = point.value;
        }
        self.pending.push(Pending {
            point,
            allocator,
            chunks: scratch.chunks,
            timing,
        });
        match &mut self.deferred {
            Some(points) => points.push(point),
            None => self.flush(),
        }
        for command in &list.commands {
            if let commands::Command::Barrier(Barrier {
                resource: Resource::Buffer(buffer),
                handoff,
                ..
            }) = command
            {
                self.buffers.get_mut(*buffer)?.owner = handoff.map_or(queue, |h| h.to);
            }
        }
        Ok(point)
    }
    /// Commit the batch (one queue's recorded command buffers) in one native call,
    /// after the other queue's and earlier commits' points it waits for, and signal
    /// its last point. Earlier buffers of the batch need no event: every recording's
    /// first encoder orders itself after all prior work on its queue.
    pub(super) fn flush(&mut self) {
        let Some(first) = self.batch.first() else {
            return;
        };
        let queue = first.point.queue;
        let mut batch = std::mem::take(&mut self.batch);
        let last = batch.last().map_or(0, |b| b.point.value);
        let mut dependencies = [0; 2];
        for batched in &batch {
            for (value, wait) in dependencies.iter_mut().zip(batched.waits) {
                *value = (*value).max(wait);
            }
        }
        dependencies[queue.index()] = dependencies[queue.index()].min(self.flushed[queue.index()]);
        if std::mem::take(&mut self.residency_dirty) {
            self.residency.commit();
        }
        let native_queue = &self.queues[queue.index()];
        for (index, value) in dependencies.into_iter().enumerate() {
            if value > 0 {
                native_queue
                    .waitForEvent_value(ProtocolObject::from_ref(&*self.events[index]), value);
            }
        }
        let buffers: Vec<_> = batch.iter().map(|b| NonNull::from(&*b.cmd)).collect();
        // SAFETY: ended, never-committed command buffers in recording order; the pointer
        // array lives across the call and the queue retains them until they complete.
        unsafe { native_queue.commit_count(NonNull::from(&buffers[0]), buffers.len()) };
        native_queue
            .signalEvent_value(ProtocolObject::from_ref(&*self.events[queue.index()]), last);
        self.flushed[queue.index()] = last;
        self.submission_stats.queue_submissions += 1;
        self.submission_stats.command_lists += batch.len() as u64;
        batch.clear();
        self.batch = batch;
    }
    /// Wait for a submission, release its recording storage and return its pass
    /// timings (GPU timestamps on the shared device clock). Inside a deferred scope it
    /// neither waits nor takes the timings, like Vulkan: later scoped submissions wait
    /// for the point on the GPU, and the scope's completion delivers its timings.
    pub fn resolve(&mut self, point: TimelinePoint) -> Result<Vec<PassTiming>> {
        if let Some(scope) = &self.deferred {
            if !scope.contains(&point) && !self.pending.iter().any(|p| p.point == point) {
                return Err(GpuError::Contract("Unknown deferred submission".into()));
            }
            let wait = &mut self.deferred_waits[point.queue.index()];
            *wait = (*wait).max(point.value);
            return Ok(Vec::new());
        }
        Device::wait(self, point)?;
        self.reclaim()?;
        Ok(self.take_timings(point))
    }
    /// Start a measurement boundary: only later, resolved submissions contribute.
    pub fn reset_timing_window(&mut self) {
        self.timing_window.reset(self.submitted);
    }
    /// Completed submission span since the reset; None while any is unresolved.
    /// Both queues share one GPU clock, so mixed-queue spans are always defined.
    pub fn timing_window(&self) -> Option<commands::TimestampSpan> {
        if self
            .pending
            .iter()
            .any(|p| self.timing_window.includes(p.point))
        {
            return None;
        }
        self.timing_window.span(true)
    }
    /// Like `VulkanDevice::bind_acceleration_structure`; unreachable until the RT tier
    /// lands, since `info().ray_tracing` is false and no structure can be created.
    pub fn bind_acceleration_structure(
        &mut self,
        _structure: AccelerationStructure,
    ) -> Result<HeapIndex> {
        Err(GpuError::Contract(
            "Acceleration structures are not available on the Metal backend yet".into(),
        ))
    }
    /// Time between this window's submission intervals; overlap counts once.
    pub fn timing_window_gaps(&self) -> Option<f64> {
        self.timing_window()?;
        self.timing_window.gaps(true)
    }
    /// Cumulative host time spent in explicit GPU waits on this device.
    pub fn host_wait_stats(&self) -> vulkan::HostWaitStats {
        self.waits.snapshot()
    }

    /// Pass timings of a completed, reclaimed submission (once).
    pub(super) fn take_timings(&mut self, point: TimelinePoint) -> Vec<PassTiming> {
        self.timings
            .iter()
            .position(|(p, _)| *p == point)
            .and_then(|at| self.timings.remove(at))
            .map(|(_, passes)| passes)
            .unwrap_or_default()
    }

    /// Return completed allocators, scratch chunks and timestamp heaps to their pools,
    /// keeping each submission's pass timings until it is resolved.
    pub(super) fn reclaim(&mut self) -> Result<()> {
        let done = Device::completed(self)?;
        let mut index = 0;
        while index < self.pending.len() {
            let point = self.pending[index].point;
            if done[point.queue.index()] < point.value {
                index += 1;
                continue;
            }
            let pending = self.pending.swap_remove(index);
            pending.allocator.reset();
            self.allocators.push(pending.allocator);
            self.scratch.recycle(pending.chunks);
            if let Some(timing) = pending.timing {
                let resolved = timestamps::resolve(&self.device, &timing, point.queue);
                self.timestamp_pool.recycle(&timing);
                let (passes, span) = resolved?;
                self.timing_window.observe(point, span);
                if self.timings.len() == RESOLVED_TIMINGS {
                    self.timings.pop_front();
                }
                self.timings.push_back((point, passes));
            }
        }
        Ok(())
    }
}

impl Device for MetalDevice {
    fn queues(&self) -> [descriptors::QueueInfo; 2] {
        [Queue::Graphics, Queue::Compute].map(|id| descriptors::QueueInfo {
            id,
            dedicated: true,
            timestamps: descriptors::TimestampCaps {
                valid_bits: 64,
                period_ns: (1e9 / self.device.queryTimestampFrequency() as f64) as f32,
            },
        })
    }
    fn create_buffer(&mut self, desc: &descriptors::BufferDesc) -> Result<Buffer> {
        MetalDevice::create_buffer(self, desc.size, desc.vk_usage(), desc.host_visible)
    }
    fn create_image(&mut self, desc: &descriptors::ImageDesc) -> Result<Image> {
        MetalDevice::create_image(
            self,
            desc.size[0],
            desc.size[1],
            desc.format.vk(),
            desc.vk_usage(),
        )
    }
    fn create_acceleration_structure(
        &mut self,
        _desc: &acceleration::AccelerationDesc,
    ) -> Result<AccelerationStructure> {
        Err(GpuError::Contract(
            "Acceleration structures are not available on the Metal backend yet".into(),
        ))
    }
    fn create_pipeline(
        &mut self,
        cooked: &cooked::CookedPipeline,
        artifacts: &std::path::Path,
    ) -> Result<Pipeline> {
        MetalDevice::create_pipeline(self, cooked, artifacts)
    }
    fn retire_pipeline(&mut self, pipeline: Pipeline) -> Result<()> {
        MetalDevice::retire_pipeline(self, pipeline)
    }
    fn heap(&mut self) -> &mut impl BindlessHeap {
        self
    }
    fn begin(&self, queue: Queue) -> Recorder {
        MetalDevice::begin(self, queue)
    }
    fn submit(&mut self, commands: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        MetalDevice::submit(self, commands, waits)
    }
    fn completed(&self) -> Result<[u64; 2]> {
        Ok([
            self.events[0].signaledValue(),
            self.events[1].signaledValue(),
        ])
    }
    fn wait(&self, point: TimelinePoint) -> Result<()> {
        if point.value > self.submitted[point.queue.index()] {
            return Err(GpuError::Contract(
                "Cannot wait on an unsubmitted timeline point".into(),
            ));
        }
        if point.value > self.flushed[point.queue.index()] {
            return Err(GpuError::Contract(
                "End the frame scope before waiting on its recorded work".into(),
            ));
        }
        let event = &self.events[point.queue.index()];
        if self
            .waits
            .measure(|| event.waitUntilSignaledValue_timeoutMS(point.value, WAIT_TIMEOUT_MS))
        {
            Ok(())
        } else {
            Err(GpuError::Backend(anyhow::anyhow!(
                "Timeline wait for {point:?} timed out after {WAIT_TIMEOUT_MS} ms"
            )))
        }
    }
    fn retire(&mut self, resource: Resource) -> Result<()> {
        let retired = match resource {
            Resource::Buffer(b) => Retired::Buffer(self.buffers.remove(b)?),
            Resource::Image(i) => Retired::Image(self.images.remove(i)?),
            Resource::AccelerationStructure(_) => return Err(GpuError::StaleHandle),
        };
        self.retired.push(retired);
        Ok(())
    }
    fn collect(&mut self) -> Result<()> {
        native::pool(|| self.collect_now())
    }
}

impl MetalDevice {
    /// Release completed retirements: table slots, residency and native objects.
    fn collect_now(&mut self) -> Result<()> {
        self.reclaim()?;
        let done = Device::completed(self)?;
        let mut index = 0;
        while index < self.retired.len() {
            let last = match &self.retired[index] {
                Retired::Buffer(b) => b.last,
                Retired::Image(i) => i.last,
                Retired::Pipeline { last, .. } => *last,
            };
            if (0..2).any(|q| done[q] < last[q]) {
                index += 1;
                continue;
            }
            match self.retired.swap_remove(index) {
                Retired::Buffer(b) => {
                    if let Some(slot) = b.heap {
                        self.tables.buffers.release(slot);
                    }
                    self.residency
                        .removeAllocation(ProtocolObject::from_ref(&*b.raw.buffer));
                    self.residency_dirty = true;
                }
                Retired::Image(i) => {
                    if let Some(slot) = i.heap {
                        self.tables.textures.release(slot);
                    }
                    if let Some(slot) = i.raw.storage {
                        self.tables.storage_images.release(slot);
                    }
                    self.residency
                        .removeAllocation(ProtocolObject::from_ref(&*i.raw.texture));
                    self.residency_dirty = true;
                }
                Retired::Pipeline { .. } => {}
            }
        }
        Ok(())
    }
}

impl BindlessHeap for MetalDevice {
    fn insert_image(&mut self, image: Image) -> Result<HeapIndex> {
        self.sampled_image(image, false)
    }
    fn insert_buffer(&mut self, buffer: Buffer) -> Result<HeapIndex> {
        self.storage_buffer(buffer)
    }
    fn insert_acceleration_structure(
        &mut self,
        _structure: AccelerationStructure,
    ) -> Result<HeapIndex> {
        Err(GpuError::Contract(
            "Acceleration structures are not available on the Metal backend yet".into(),
        ))
    }
    fn insert_storage_image(&mut self, image: Image) -> Result<HeapIndex> {
        self.storage_image(image)
    }
}

impl Drop for MetalDevice {
    fn drop(&mut self) {
        // Native objects are reference counted, but the GPU may still read memory we
        // own (tables, scratch, resources): wait for every submitted value first.
        native::pool(|| {
            for queue in [Queue::Graphics, Queue::Compute] {
                let value = self.flushed[queue.index()];
                if value > 0 {
                    let _ = self.events[queue.index()]
                        .waitUntilSignaledValue_timeoutMS(value, WAIT_TIMEOUT_MS);
                }
            }
            for queue in &self.queues {
                queue.removeResidencySet(&self.residency);
            }
        });
    }
}
