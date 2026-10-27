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
//! Not yet available: timestamps (`resolve` returns no pass timings), acceleration
//! structures, mesh/task stages, `ClearDepth`, presentation and shared surfaces.
mod abi;
mod encode;
mod native;
mod pipeline;
mod resources;
mod scratch;
mod tables;
#[cfg(test)]
mod tests;

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
}

/// Metal device capabilities recorded at creation.
#[derive(Debug, Clone)]
pub struct MetalInfo {
    /// Native device name, e.g. "Apple M5 Pro".
    pub name: String,
    /// Apple GPU family 10 or later (hardware ray tracing and mesh shading).
    pub apple10: bool,
    /// Ray-query tier exposed by this backend; false until AS support lands.
    pub ray_tracing: bool,
    /// Mesh/task stages exposed by this backend; false until the mesh tier lands.
    pub mesh_shading: bool,
    /// BC1-BC7 sampled textures (always true on Apple silicon macOS).
    pub texture_compression_bc: bool,
    /// Sampled-image table capacity.
    pub sampled_image_capacity: u32,
    /// Storage-buffer table capacity.
    pub storage_buffer_capacity: u32,
    /// Recommended working-set size in MiB (unified memory).
    pub working_set_mib: u64,
}

/// Owned Metal 4 device, arenas, converter descriptor tables and queue timelines.
pub struct MetalDevice {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queues: [Retained<ProtocolObject<dyn MTL4CommandQueue>>; 2],
    events: [Retained<ProtocolObject<dyn MTLSharedEvent>>; 2],
    residency: Retained<ProtocolObject<dyn MTLResidencySet>>,
    residency_dirty: bool,
    compiler: Retained<ProtocolObject<dyn MTL4Compiler>>,
    table: Retained<ProtocolObject<dyn MTL4ArgumentTable>>,
    tables: tables::Tables,
    scratch: scratch::Pool,
    allocators: Vec<Retained<ProtocolObject<dyn MTL4CommandAllocator>>>,
    buffers: Arena<BufferKind, Owned<NativeBuffer>>,
    images: Arena<ImageKind, Owned<NativeImage>>,
    pipelines: Arena<PipelineKind, NativePipeline>,
    submitted: [u64; 2],
    retired: Vec<Retired>,
    pending: Vec<Pending>,
    info: MetalInfo,
}

impl MetalDevice {
    /// Create the system default Metal 4 device. `_window_extensions` keeps the Vulkan
    /// constructor's signature for backend-neutral callers; Metal needs none.
    /// Raster-only like [`crate::vulkan::VulkanDevice::new_raster`].
    pub fn new_raster(
        _window_extensions: &[std::ffi::CString],
        _cache_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
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
        let compiler = device
            .newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new())
            .map_err(|e| ns_error("MTL4 compiler", &e))?;
        let table_desc = MTL4ArgumentTableDescriptor::new();
        table_desc.setMaxBufferBindCount(abi::ARGUMENT_TABLE_BUFFERS);
        let table = device
            .newArgumentTableWithDescriptor_error(&table_desc)
            .map_err(|e| ns_error("argument table", &e))?;
        let tables = tables::Tables::new(&device)?;
        for buffer in tables.buffers() {
            residency.addAllocation(ProtocolObject::from_ref(buffer));
        }
        let info = MetalInfo {
            name: device.name().to_string(),
            apple10: device.supportsFamily(MTLGPUFamily::Apple10),
            ray_tracing: false,
            mesh_shading: false,
            texture_compression_bc: device.supportsBCTextureCompression(),
            sampled_image_capacity: abi::TEXTURES,
            storage_buffer_capacity: abi::BUFFERS,
            working_set_mib: device.recommendedMaxWorkingSetSize() / (1024 * 1024),
        };
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
            retired: Vec::new(),
            pending: Vec::new(),
            info,
        })
    }

    /// Capabilities recorded at creation.
    pub fn info(&self) -> &MetalInfo {
        &self.info
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
        let raw = resources::buffer(&self.device, size, usage, host)?;
        self.residency
            .addAllocation(ProtocolObject::from_ref(&*raw.buffer));
        self.residency_dirty = true;
        self.buffers.insert(Owned {
            raw,
            last: [0; 2],
            heap: None,
        })
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
        let raw = resources::image(&self.device, size, format, mip_levels, usage)?;
        self.residency
            .addAllocation(ProtocolObject::from_ref(&*raw.texture));
        self.residency_dirty = true;
        self.images.insert(Owned {
            raw,
            last: [0; 2],
            heap: None,
        })
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
        let native = pipeline::create(&self.device, &self.compiler, cooked, artifacts)?;
        self.pipelines.insert(native)
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

    /// Submit one list with timeline dependencies. Returns a monotonic signal point.
    /// Bindless reachability is conservative: all published resources are retained.
    pub fn submit(&mut self, list: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        for wait in waits {
            if wait.value > self.submitted[wait.queue.index()] {
                return Err(GpuError::Contract("Wait targets unsubmitted work".into()));
            }
        }
        self.reclaim()?;
        let queue = list.queue;
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
        let encoded = self.encode(&cmd, &list, &mut scratch, &mut created);
        cmd.endCommandBuffer();
        for chunk in &created {
            self.residency
                .addAllocation(ProtocolObject::from_ref(&**chunk));
            self.residency_dirty = true;
        }
        if let Err(error) = encoded {
            // Nothing was committed: the allocator and scratch are immediately reusable.
            allocator.reset();
            self.allocators.push(allocator);
            self.scratch.recycle(scratch.chunks);
            return Err(error);
        }
        if std::mem::take(&mut self.residency_dirty) {
            self.residency.commit();
        }
        let native_queue = &self.queues[queue.index()];
        for wait in waits {
            native_queue.waitForEvent_value(
                ProtocolObject::from_ref(&*self.events[wait.queue.index()]),
                wait.value,
            );
        }
        let buffers = [NonNull::from(&*cmd)];
        // SAFETY: one ended, never-committed command buffer; the pointer array lives
        // across the call and the queue retains the buffer until it completes.
        unsafe { native_queue.commit_count(NonNull::from(&buffers[0]), 1) };
        native_queue.signalEvent_value(
            ProtocolObject::from_ref(&*self.events[queue.index()]),
            point.value,
        );
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
        });
        Ok(point)
    }
    /// Wait for a submission and release its recording storage. Metal counter
    /// sampling is not wired yet, so no pass timings are returned.
    pub fn resolve(&mut self, point: TimelinePoint) -> Result<Vec<PassTiming>> {
        Device::wait(self, point)?;
        self.reclaim()?;
        Ok(Vec::new())
    }

    /// Return completed allocators and scratch chunks to their pools.
    fn reclaim(&mut self) -> Result<()> {
        let done = Device::completed(self)?;
        let mut index = 0;
        while index < self.pending.len() {
            let point = self.pending[index].point;
            if done[point.queue.index()] >= point.value {
                let pending = self.pending.swap_remove(index);
                pending.allocator.reset();
                self.allocators.push(pending.allocator);
                self.scratch.recycle(pending.chunks);
            } else {
                index += 1;
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
                valid_bits: 0,
                period_ns: 0.,
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
        if self.events[point.queue.index()]
            .waitUntilSignaledValue_timeoutMS(point.value, WAIT_TIMEOUT_MS)
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
        for queue in [Queue::Graphics, Queue::Compute] {
            let value = self.submitted[queue.index()];
            if value > 0 {
                let _ = self.events[queue.index()]
                    .waitUntilSignaledValue_timeoutMS(value, WAIT_TIMEOUT_MS);
            }
        }
        for queue in &self.queues {
            queue.removeResidencySet(&self.residency);
        }
    }
}
