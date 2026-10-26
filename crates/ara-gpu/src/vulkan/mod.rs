//! Vulkan 1.3 backend. Raw Vulkan access is confined to backend integrations.
mod acceleration;
mod cache;
mod completion;
mod context;
mod disposal;
use ash::vk::TaggedStructure;
pub use completion::Completion;
mod encode;
pub mod external;
mod guard;
mod heap;
mod image_copy;
mod lifetime;
mod pipeline;
mod preparation;
pub use preparation::{
    BufferPreparation, ImagePreparation, InitialData, PreparationId, PreparationKind,
    PreparedResources, ResourcePreparation,
};
mod queries;
#[cfg(test)]
mod query_tests;
mod recording;
mod reload;
pub use reload::ShaderStatus;
mod surface;
mod timestamps;
mod ui_readback;
mod waits;
pub use waits::HostWaitStats;

pub use crate::commands::TimestampSpan;
pub use crate::commands::{Attachment, Command, CommandList, Draw, PassTiming, Recorder};
use crate::{arena::Arena, *};
use context::{BufferAllocation, ImageAllocation};
pub use context::{DeviceInfo, Diagnostic, Gpu as Context, image_barrier};
pub use heap::DescriptorUsage;
use heap::Heap;
use std::{ffi::CString, path::PathBuf};

struct Owned<V> {
    raw: V,
    last: [u64; 2],
    heap: Option<HeapIndex>,
    owner: Queue,
}
enum Retired {
    Buffer(Owned<BufferAllocation>),
    Image(Owned<ImageAllocation>),
    Acceleration(Owned<acceleration::NativeAcceleration>),
    Pipeline(pipeline::NativePipeline, [u64; 2]),
}
impl Retired {
    fn last(&self) -> [u64; 2] {
        match self {
            Self::Buffer(v) => v.last,
            Self::Image(v) => v.last,
            Self::Acceleration(v) => v.last,
            Self::Pipeline(_, last) => *last,
        }
    }
}

/// Owned Vulkan device, arenas, immutable heap slots and persistent pipeline cache.
pub struct VulkanDevice {
    /// Native integration context. Unsafe ash calls remain the integration's responsibility.
    context: Context,
    buffers: Arena<BufferKind, Owned<BufferAllocation>>,
    images: Arena<ImageKind, Owned<ImageAllocation>>,
    accelerations: Arena<AccelerationStructureKind, Owned<acceleration::NativeAcceleration>>,
    pipelines: Arena<PipelineKind, pipeline::NativePipeline>,
    heap: Heap,
    timelines: [vk::Semaphore; 2],
    submitted: [u64; 2],
    retired: Vec<Retired>,
    cache: vk::PipelineCache,
    cache_path: PathBuf,
    pending: Vec<recording::Pending>,
    query_cache: queries::Cache,
    #[cfg(test)]
    query_pool_creations: usize,
    shader_worker: Option<reload::Worker>,
    disposal: Option<disposal::Disposal>,
    preparation: Option<preparation::Worker>,
    deferred: Option<Vec<TimelinePoint>>,
    completion_pending: bool,
    deferred_waits: [u64; 2],
    scene_queue: bool,
    clock: timestamps::Clock,
    timing_window: timestamps::Window,
}

impl VulkanDevice {
    /// Inspect reserved descriptor capacity at a content/test boundary. Collect
    /// completed retirements first when checking a replacement for resource leaks.
    pub fn descriptor_usage(&self) -> DescriptorUsage {
        self.heap.usage(&self.context)
    }
    /// Read-only native integration access; ownership remains with this device.
    pub fn context(&self) -> &Context {
        &self.context
    }
    /// Create a debug/sync-validated device. Empty extensions create a headless device.
    pub fn new(window_extensions: &[CString], cache_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::from_context(Context::new(window_extensions)?, cache_dir.into())
    }

    /// Create a raster-only logical device, even on hardware that supports RT.
    /// Ray-query/AS extensions, features and heap capacity are omitted; acceleration
    /// structure allocation is rejected before creating any native RT resources.
    pub fn new_raster(
        window_extensions: &[CString],
        cache_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        Self::from_context(
            Context::with_ray_tracing(window_extensions, false)?,
            cache_dir.into(),
        )
    }

    fn from_context(context: Context, cache_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&cache_dir).map_err(anyhow::Error::from)?;
        let uuid = context
            .info
            .pipeline_cache_uuid
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let cache_path = cache_dir.join(format!(
            "vulkan-{}-{}-{uuid}-{}.bin",
            context.info.vendor_id, context.info.device_id, context.info.driver_version
        ));
        let bytes = std::fs::read(&cache_path).unwrap_or_default();
        let seed = if cache::compatible(&bytes, &context.info) {
            bytes
        } else {
            Vec::new()
        };
        let mut guard = guard::Objects::new(&context.device);
        // SAFETY: Cache bytes are device/driver scoped; invalid data is discarded.
        let cache = unsafe {
            context
                .device
                .create_pipeline_cache(
                    &vk::PipelineCacheCreateInfo::default().initial_data(&seed),
                    None,
                )
                .or_else(|_| {
                    context
                        .device
                        .create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None)
                })
        }
        .map_err(anyhow::Error::from)?;
        guard.push(guard::Object::Cache(cache));
        let mut timelines = [vk::Semaphore::null(); 2];
        for semaphore in &mut timelines {
            let mut ty =
                vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
            // SAFETY: Device is live; each semaphore has independent monotonic values.
            *semaphore = unsafe {
                context
                    .device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default().push(&mut ty), None)
            }
            .map_err(anyhow::Error::from)?;
            guard.push(guard::Object::Semaphore(*semaphore));
        }
        let clock = timestamps::Clock::new(&context)?;
        let disposal = disposal::Disposal::new(context.device.clone())?;
        let heap = Heap::new(&context)?;
        let shader_worker = if std::env::var_os("ARA_SHADER_DIR").is_some() {
            Some(reload::Worker::new(
                context.device.clone(),
                context.info.mesh_shading,
                heap.layout,
            )?)
        } else {
            None
        };
        guard.disarm();
        Ok(Self {
            context,
            buffers: Arena::default(),
            images: Arena::default(),
            accelerations: Arena::default(),
            pipelines: Arena::default(),
            heap,
            timelines,
            submitted: [0; 2],
            retired: Vec::new(),
            cache,
            cache_path,
            pending: Vec::new(),
            query_cache: queries::Cache::default(),
            #[cfg(test)]
            query_pool_creations: 0,
            shader_worker,
            disposal: Some(disposal),
            preparation: None,
            deferred: None,
            completion_pending: false,
            deferred_waits: [0; 2],
            scene_queue: false,
            clock,
            timing_window: Default::default(),
        })
    }

    /// Allocate a buffer. Host memory is coherent and may be mapped after completion.
    pub fn create_buffer(
        &mut self,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: bool,
    ) -> Result<Buffer> {
        if size == 0 || usage.is_empty() {
            return Err(GpuError::Contract("Empty buffer".into()));
        }
        let usage = if host {
            usage | vk::BufferUsageFlags::TRANSFER_DST
        } else {
            usage
        };
        let raw = self.context.buffer(size, usage, host)?;
        self.buffers.insert(Owned {
            raw,
            last: [0; 2],
            heap: None,
            owner: Queue::Graphics,
        })
    }
    /// Allocate coherent transfer readback memory, preferring CPU-cached RAM.
    /// This avoids slow CPU reads from uncached host-visible/BAR device memory;
    /// devices without cached coherent memory retain the coherent fallback.
    pub fn create_readback_buffer(&mut self, size: u64) -> Result<Buffer> {
        if size == 0 {
            return Err(GpuError::Contract("Empty readback buffer".into()));
        }
        let raw = self.context.readback_buffer(size)?;
        self.buffers.insert(Owned {
            raw,
            last: [0; 2],
            heap: None,
            owner: Queue::Graphics,
        })
    }

    /// Allocate a single-level image; graph transients use the same owned path.
    pub fn create_image(
        &mut self,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image> {
        self.create_mip_image([width, height], format, 1, usage)
    }

    /// Allocates a sampled texture with native mip levels, including compressed BC
    /// formats. UploadImage and ReadImage transfer all levels, finest first.
    pub fn create_mip_image(
        &mut self,
        size: [u32; 2],
        format: vk::Format,
        mip_levels: u32,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image> {
        let desc = context::allocation::ImageDesc::new(size, format, mip_levels, usage);
        self.context.validate_image(&desc)?;
        let [width, height] = size;
        let aspect = desc.aspect;
        let raw = self
            .context
            .image(width, height, format, mip_levels, usage, aspect)?;
        self.images.insert(Owned {
            raw,
            last: [0; 2],
            heap: None,
            owner: Queue::Graphics,
        })
    }
    /// Write a coherent buffer after every preceding use has completed.
    #[track_caller]
    pub fn upload(&self, buffer: Buffer, bytes: &[u8]) -> Result<()> {
        self.upload_range(buffer, 0, bytes)
    }
    /// Write a byte range without copying unchanged rows. Like `upload`, this
    /// requires coherent host storage and completion of every preceding GPU use.
    #[track_caller]
    pub fn upload_range(&self, buffer: Buffer, offset: u64, bytes: &[u8]) -> Result<()> {
        let b = self.buffers.get(buffer)?;
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < b.last[q]) {
            return Err(GpuError::Contract(format!(
                "Upload would overwrite in-flight storage at {} ({buffer:?}; completed {completed:?}, last {:?})",
                std::panic::Location::caller(),
                b.last
            )));
        }
        self.context.upload_range(b.raw, offset, bytes)?;
        Ok(())
    }
    /// Read coherent mapped storage after timeline completion and a HostRead barrier.
    pub fn read(&self, buffer: Buffer) -> Result<Vec<u8>> {
        let b = self.buffers.get(buffer)?;
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < b.last[q]) {
            return Err(GpuError::Contract("Readback still in flight".into()));
        }
        Ok(self.context.read(b.raw)?)
    }
    /// Publish a sampled image into a fresh immutable descriptor slot.
    pub fn sampled_image(&mut self, image: Image, depth: bool) -> Result<HeapIndex> {
        let value = self.images.get_mut(image)?;
        if !value.raw.usage.contains(vk::ImageUsageFlags::SAMPLED) {
            return Err(GpuError::Contract(
                "Image was not allocated for sampling".into(),
            ));
        }
        if let Some(index) = value.heap {
            return Ok(index);
        }
        let index = self.heap.image(&self.context, value.raw.view, depth)?;
        value.heap = Some(index);
        Ok(index)
    }
    /// Publish a storage buffer into a fresh immutable descriptor slot.
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
        let index = self.heap.buffer(&self.context, value.raw)?;
        value.heap = Some(index);
        Ok(index)
    }
    /// Publish a storage-image view. Slot lifetime follows image retirement.
    pub fn storage_image(&mut self, image: Image) -> Result<HeapIndex> {
        let value = self.images.get(image)?;
        if !value.raw.usage.contains(vk::ImageUsageFlags::STORAGE) {
            return Err(GpuError::Contract(
                "Image was not allocated for shader storage".into(),
            ));
        }
        self.heap
            .storage_image(&self.context, image, value.raw.view)
    }
    /// Create a pipeline exclusively from a versioned, hashed offline cook.
    pub fn create_pipeline(
        &mut self,
        cooked: &cooked::CookedPipeline,
        artifacts: &std::path::Path,
    ) -> Result<Pipeline> {
        crate::cpu_scope!("GPU.pipeline_create");
        let native = pipeline::create(
            &self.context,
            self.heap.layout,
            self.cache,
            cooked,
            artifacts,
        )?;
        let handle = self.pipelines.insert(native)?;
        if let Some(worker) = &mut self.shader_worker {
            worker.track(handle, cooked, artifacts)?;
        }
        Ok(handle)
    }
    /// Invalidate a cooked pipeline identity and defer native destruction until all queues complete.
    pub fn retire_pipeline(&mut self, pipeline: Pipeline) -> Result<()> {
        if let Some(worker) = &self.shader_worker {
            worker.forget(pipeline);
        }
        self.retired.push(Retired::Pipeline(
            self.pipelines.remove(pipeline)?,
            self.submitted,
        ));
        Ok(())
    }
    /// Start an owned CPU recorder. Submission validates all resource handles.
    pub fn begin(&self, queue: Queue) -> Recorder {
        Recorder::new(queue)
    }
    /// Native image handle for presentation; its lifetime stays owned by this device.
    pub fn native_image(&self, image: Image) -> Result<vk::Image> {
        Ok(self.images.get(image)?.raw.handle)
    }
    /// Native image view for native integrations.
    pub fn image_view(&self, image: Image) -> Result<vk::ImageView> {
        Ok(self.images.get(image)?.raw.view)
    }
    /// Native mip-zero extent, including render-size ID targets under upscaling.
    pub fn image_extent(&self, image: Image) -> Result<[u32; 2]> {
        let image = self.images.get(image)?.raw;
        Ok([image.width, image.height])
    }
    /// Return structured driver validation diagnostics recorded since device creation.
    pub fn diagnostics(&self) -> Vec<context::Diagnostic> {
        self.context
            .validation
            .messages
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }
    /// Check both API and synchronization validation.
    pub fn assert_clean(&self) -> Result<()> {
        self.context.assert_clean()?;
        Ok(())
    }
    fn family(&self, queue: Queue) -> u32 {
        match queue {
            Queue::Graphics => self.context.info.graphics_family,
            Queue::Compute => self.context.info.compute_family,
        }
    }
}
