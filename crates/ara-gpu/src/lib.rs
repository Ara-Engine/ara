#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Ara's owned GPU resources, explicit queues and offline pipeline contract.
pub mod acceleration;
pub mod arena;
pub mod commands;
pub mod cooked;
pub mod descriptors;
#[cfg(target_os = "macos")]
pub mod metal;
pub mod shared;
pub mod vulkan;

pub use ash::vk;
use serde::{Deserialize, Serialize};

/// A logical execution queue; backend family indices never enter the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Queue {
    /// Raster, transfer and presentation work.
    Graphics,
    /// Dedicated compute family, independent of graphics.
    Compute,
}
impl Queue {
    /// Index in the fixed two-queue completion vector.
    pub const fn index(self) -> usize {
        match self {
            Self::Graphics => 0,
            Self::Compute => 1,
        }
    }
}

/// Completion on one queue's monotonic timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelinePoint {
    /// Signalling queue.
    pub queue: Queue,
    /// Monotonically increasing value, starting at one.
    pub value: u64,
}

/// Structured runtime or driver failure.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// A CPU resource identity has expired or belongs to another device.
    #[error("ARA_GPU_STALE_HANDLE")]
    StaleHandle,
    /// Driver capability, allocation, recording or submission failure.
    #[error("ARA_GPU_BACKEND: {0}")]
    Backend(#[from] anyhow::Error),
    /// Caller violated a typed API precondition.
    #[error("ARA_GPU_CONTRACT: {0}")]
    Contract(String),
    /// A shared surface (cross-process GPU memory) cannot be exported or imported:
    /// unsupported platform or extension, or a different GPU/driver on each side.
    #[error("ARA_GPU_EXTERNAL: {0}")]
    External(String),
}
/// GPU operations return structured failures.
pub type Result<T> = std::result::Result<T, GpuError>;

/// `ARA_GPU_EXTERNAL`: a shared surface cannot be exported or imported (named-code
/// slot 6006, documented in `ara-diag/codes/gpu.md`).
pub const GPU_EXTERNAL: ara_diag::Code = ara_diag::Code(6006);

impl From<&GpuError> for ara_diag::Diagnostic {
    /// The error as a bus diagnostic under its `ARA_GPU_*` named code.
    fn from(e: &GpuError) -> ara_diag::Diagnostic {
        use ara_diag::codes;
        let (code, message) = match e {
            GpuError::StaleHandle => (
                codes::GPU_STALE_HANDLE,
                "a GPU handle was used after its resource was destroyed".to_owned(),
            ),
            GpuError::Backend(err) => (codes::GPU_BACKEND, format!("{err:#}")),
            GpuError::Contract(m) => (codes::GPU_CONTRACT, m.clone()),
            GpuError::External(m) => (GPU_EXTERNAL, m.clone()),
        };
        ara_diag::Diagnostic::error(code, message)
    }
}

/// Buffer resource tag.
pub enum BufferKind {}
/// Image resource tag.
pub enum ImageKind {}
/// Pipeline resource tag.
pub enum PipelineKind {}
/// Acceleration structure resource tag.
pub enum AccelerationStructureKind {}
/// Generation checked buffer identity, distinct from a shader heap index.
pub type Buffer = arena::Handle<BufferKind>;
/// Generation checked image identity.
pub type Image = arena::Handle<ImageKind>;
/// Generation checked BLAS/TLAS identity.
pub type AccelerationStructure = arena::Handle<AccelerationStructureKind>;
/// Generation checked pipeline identity.
pub type Pipeline = arena::Handle<PipelineKind>;

/// The shared 32-byte root constant ABI; contents belong to the cooked shader.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RootConstants {
    /// Eight 32-bit words, including SM6.6 style heap indices.
    pub words: [u32; 8],
}

/// Shader-visible descriptor identity. Slots never change while referenced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapIndex {
    /// Index in the sampled-image or storage-buffer table.
    pub index: u32,
}

/// A whole resource tracked by the graph and retirement system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// Buffer storage.
    Buffer(Buffer),
    /// Image storage and view.
    Image(Image),
    /// Bottom/top level tracing storage.
    AccelerationStructure(AccelerationStructure),
}

/// Semantic resource usage lowered to backend stages, masks and layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// First use of uninitialized transient storage.
    Undefined,
    /// Vertex input buffer.
    Vertex,
    /// Index input buffer.
    Index,
    /// GPU-generated dispatch/draw arguments, also readable by shaders.
    Indirect,
    /// Sampled image / readonly shader storage.
    ShaderRead,
    /// Compute unordered read/write.
    Storage,
    /// Raster color attachment read/write.
    Color,
    /// Raster depth attachment read/write.
    Depth,
    /// Copy source.
    CopySource,
    /// Copy destination.
    CopyDestination,
    /// Coherent mapped readback after GPU completion.
    HostRead,
    /// Coherent CPU upload completed before submission.
    HostWrite,
    /// Acceleration structure construction destination.
    AsBuild,
    /// Acceleration structure or triangle/instance input read during a build.
    AsInput,
    /// Shader ray-query read.
    AsRead,
}
impl Access {
    /// Whether this usage can change resource contents.
    pub const fn writes(self) -> bool {
        matches!(
            self,
            Self::Storage
                | Self::Color
                | Self::Depth
                | Self::CopyDestination
                | Self::AsBuild
                | Self::HostWrite
        )
    }
}

/// Explicit queue ownership edge. Release/acquire carry identical ranges/layouts.
#[derive(Debug, Clone, Copy)]
pub struct Handoff {
    /// Previous owner.
    pub from: Queue,
    /// Next owner.
    pub to: Queue,
    /// True in the source command buffer; false in the destination.
    pub release: bool,
}
/// Whole-resource synchronization compiled from declared accesses.
#[derive(Debug, Clone, Copy)]
pub struct Barrier {
    /// Resource whose usage changes.
    pub resource: Resource,
    /// Previous usage.
    pub before: Access,
    /// Following usage.
    pub after: Access,
    /// Queue transfer, or same-queue dependency.
    pub handoff: Option<Handoff>,
}

/// Backend-neutral resource, recording and lifetime surface, shared with Metal.
pub trait Device {
    /// Queue capabilities for graphics and dedicated compute.
    fn queues(&self) -> [descriptors::QueueInfo; 2];
    /// Allocate a typed buffer.
    fn create_buffer(&mut self, desc: &descriptors::BufferDesc) -> Result<Buffer>;
    /// Allocate a typed image.
    fn create_image(&mut self, desc: &descriptors::ImageDesc) -> Result<Image>;
    /// Allocate static triangle/instance acceleration storage.
    fn create_acceleration_structure(
        &mut self,
        desc: &acceleration::AccelerationDesc,
    ) -> Result<AccelerationStructure>;
    /// Construct a pipeline from offline artifact bytes and state.
    fn create_pipeline(
        &mut self,
        cooked: &cooked::CookedPipeline,
        artifacts: &std::path::Path,
    ) -> Result<Pipeline>;
    /// Retire a pipeline after all referencing queues complete.
    fn retire_pipeline(&mut self, pipeline: Pipeline) -> Result<()>;
    /// Publish immutable resource indices in the global heap.
    fn heap(&mut self) -> &mut impl BindlessHeap;
    /// Start owned recording with no long-lived device or ECS borrow.
    fn begin(&self, queue: Queue) -> commands::Recorder;
    /// Submit work and signal the next timeline value on its queue.
    fn submit(
        &mut self,
        commands: commands::CommandList,
        waits: &[TimelinePoint],
    ) -> Result<TimelinePoint>;
    /// Return completed values for graphics and compute.
    fn completed(&self) -> Result<[u64; 2]>;
    /// Block until a queue reaches a requested point.
    fn wait(&self, point: TimelinePoint) -> Result<()>;
    /// Invalidate a handle immediately; reclaim only after all recorded uses.
    fn retire(&mut self, resource: Resource) -> Result<()>;
    /// Reclaim resources whose referencing queues have completed.
    fn collect(&mut self) -> Result<()>;
}

/// Immutable shader-visible slots. Retire a slot through its owning resource.
/// Callers must declare every resource reachable through their root indices.
pub trait BindlessHeap {
    /// Insert a sampled image; the slot cannot be overwritten while in flight.
    fn insert_image(&mut self, image: Image) -> Result<HeapIndex>;
    /// Insert a storage buffer with an independent descriptor identity.
    fn insert_buffer(&mut self, buffer: Buffer) -> Result<HeapIndex>;
    /// Insert a top-level acceleration structure in the optional RT table.
    fn insert_acceleration_structure(
        &mut self,
        structure: AccelerationStructure,
    ) -> Result<HeapIndex>;
    /// Insert a writable image in the storage-image table.
    fn insert_storage_image(&mut self, image: Image) -> Result<HeapIndex>;
}
