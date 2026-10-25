//! Portable typed recording and timestamp results, shared by native backends.
use crate::*;
use serde::Serialize;

/// Color/depth attachment with an optional clear. None preserves prior contents.
#[derive(Clone, Copy)]
pub struct Attachment {
    /// Arena-owned target.
    pub image: Image,
    /// Clear value. Depth attachments use the first component.
    pub clear: Option<[f32; 4]>,
}
/// One indexed draw. Vertex records follow the cooked 48-byte baseline ABI.
pub struct Draw {
    /// Vertex storage.
    pub vertices: Buffer,
    /// U32 index storage.
    pub indices: Buffer,
    /// First index in the mesh buffer.
    pub first_index: u32,
    /// Number of indices.
    pub index_count: u32,
}
/// Owned high-level command, translated only after generation validation.
pub enum Command {
    /// Explicit graph dependency.
    Barrier(Barrier),
    /// Bind cooked pipeline and the global descriptor heap.
    Pipeline(Pipeline),
    /// Update eight root words.
    Constants(RootConstants),
    /// Construct a static BLAS/TLAS using its preallocated aligned scratch.
    BuildAccelerationStructure(AccelerationStructure),
    /// Dispatch compute workgroups.
    Dispatch([u32; 3]),
    /// Dispatch using three U32 workgroup counts at an aligned buffer offset.
    DispatchIndirect {
        /// Buffer allocated with native indirect usage.
        arguments: Buffer,
        /// Byte offset of the 12-byte dispatch record.
        offset: u64,
    },
    /// Begin dynamic rendering.
    BeginRendering {
        /// Color outputs.
        colors: Vec<Attachment>,
        /// Optional reversed-Z depth output.
        depth: Option<Attachment>,
        /// Render extent.
        size: [u32; 2],
    },
    /// Dynamic viewport and scissor; used for atlas quadrants.
    Viewport {
        /// Top-left pixel.
        origin: [u32; 2],
        /// Pixel extent.
        size: [u32; 2],
    },
    /// Indexed mesh draw.
    Draw(Draw),
    /// One GPU-generated non-indexed instanced draw (four u32 argument words).
    /// Vertex data is fetched from declared shader storage by SV_VertexID.
    DrawIndirect {
        /// Buffer allocated with INDIRECT_BUFFER usage.
        arguments: Buffer,
        /// Four-byte-aligned byte offset of one 16-byte draw record.
        offset: u64,
    },
    /// GPU-generated mesh workgroups, requiring the optional mesh-shader tier.
    DrawMeshTasksIndirect {
        /// Buffer with INDIRECT_BUFFER usage and three U32 group counts.
        arguments: Buffer,
        /// Four-byte-aligned offset of the 12-byte argument record.
        offset: u64,
    },
    /// Fullscreen triangle without vertex input.
    Fullscreen,
    /// Procedural instanced draw using SV_VertexID/SV_InstanceID, without vertex input.
    DrawProcedural {
        /// Vertices in each instance; nonzero.
        vertices: u32,
        /// Number of instances; nonzero.
        instances: u32,
    },
    /// End dynamic rendering.
    EndRendering,
    /// Buffer-to-image upload with tightly packed pixels.
    UploadImage {
        /// Staging buffer.
        source: Buffer,
        /// Destination image.
        destination: Image,
    },
    /// Copy a whole tightly packed buffer prefix, used for device-local uploads.
    CopyBuffer {
        /// Transfer source allocation.
        source: Buffer,
        /// Transfer destination allocation.
        destination: Buffer,
        /// Nonzero byte count, divisible by four.
        size: u64,
    },
    /// Copy a checked byte range between distinct buffers, outside rendering.
    CopyBufferRange {
        /// Transfer source allocation.
        source: Buffer,
        /// Four-byte-aligned offset into the source.
        source_offset: u64,
        /// Transfer destination allocation.
        destination: Buffer,
        /// Four-byte-aligned offset into the destination.
        destination_offset: u64,
        /// Nonzero byte count, divisible by four.
        size: u64,
    },
    /// Copy mip zero between distinct, equally sized and formatted color images.
    CopyImage {
        /// Completed transfer source.
        source: Image,
        /// Transfer destination.
        destination: Image,
    },
    /// Image-to-buffer readback with tightly packed pixels.
    ReadImage {
        /// Copy source.
        source: Image,
        /// Coherent readback destination.
        destination: Buffer,
    },
    /// Begin a measured graph pass.
    BeginPass(String),
    /// End the most recent pass.
    EndPass,
}
/// An owned CPU command list, scoped to one logical queue.
pub struct CommandList {
    pub(crate) queue: Queue,
    pub(crate) commands: Vec<Command>,
}
/// CPU recording requires no device/ECS borrow or resource cloning.
pub struct Recorder {
    pub(crate) queue: Queue,
    pub(crate) commands: Vec<Command>,
}
impl Recorder {
    pub(crate) fn new(queue: Queue) -> Self {
        Self {
            queue,
            commands: Vec::new(),
        }
    }
    /// Append a typed operation.
    pub fn push(&mut self, command: Command) {
        self.commands.push(command);
    }
    /// Record explicit graph barriers.
    pub fn barriers(&mut self, barriers: impl IntoIterator<Item = Barrier>) {
        self.commands
            .extend(barriers.into_iter().map(Command::Barrier));
    }
    /// Finish recording; resources are validated before native submission.
    pub fn finish(self) -> CommandList {
        CommandList {
            queue: self.queue,
            commands: self.commands,
        }
    }
}
/// Timestamp result for one graph pass, measured on its actual queue.
#[derive(Debug, Clone, Serialize)]
pub struct PassTiming {
    /// Graph pass label.
    pub pass: String,
    /// Execution queue.
    pub queue: Queue,
    /// Device timestamp delta in milliseconds.
    pub milliseconds: f64,
}
