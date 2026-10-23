//! GPU page marking, cached 16k shadow projections and shared-geometry raster.
mod capture;
mod init;
mod record;
mod reference;
mod update;
use super::*;
use ara_gpu::vk;
use ara_vsm::Light;
pub use reference::ShadowReference;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Projection {
    vp: Mat4,
    camera_focal: [f32; 4],
    control: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowFrame {
    projections: [Projection; 6],
    counts: [u32; 4],
    extent: [u32; 4],
    slots0: [u32; 4],
    slots1: [u32; 4],
    slots2: [u32; 4],
    geometry: [u32; 4],
    minimum: [f32; 4],
    scale: [f32; 4],
    inputs: [u32; 4],
}
/// GPU virtual shadow cache for one sun/spot/point light. A sun uses up to six
/// clipmaps, a point light six faces. The renderer consumes a scalar visibility
/// image, retaining its independent RT shadow path for comparison.
pub struct VirtualShadows {
    frame: Buffer,
    buffers: [Buffer; 8],
    slots: [u32; 8],
    pipelines: BTreeMap<String, Pipeline>,
    mask: Image,
    mask_slot: u32,
    world_slot: u32,
    normal_slot: u32,
    preview_slot: u32,
    previous: Vec<State>,
    size: [u32; 2],
    capacity: u32,
    cluster_capacity: u32,
    frame_id: u32,
    matrices: [Mat4; 6],
    revisions: [u64; 6],
    projection_count: usize,
    geometry_revision: u64,
    last: Option<ara_gpu::TimelinePoint>,
}
impl VirtualShadows {
    /// Requested owned resource bytes; excludes driver/alignment overhead and
    /// the borrowed geometry extraction.
    pub fn memory_bytes(&self) -> u64 {
        6 * 128 * 128 * 8
            + u64::from(self.capacity) * (128 * 128 * 4 + 20)
            + u64::from(self.cluster_capacity) * 16
            + 256
            + size_of::<ShadowFrame>() as u64
            + u64::from(self.size[0]) * u64::from(self.size[1]) * 16
    }
    /// Marks the submitted cache use, preventing in-flight host overwrites.
    pub fn submitted(&mut self, point: ara_gpu::TimelinePoint) {
        self.last = Some(point);
    }
    /// Scalar shadow visibility in RGB; alpha reports whether all PCF taps are resident.
    /// Returns Graphics/ShaderRead after the recorded passes complete.
    pub fn output(&self) -> Image {
        self.mask
    }
}
/// Diagnostic GPU page/cache counters; never used to submit draws.
#[derive(serde::Serialize)]
pub struct ShadowStats {
    /// Pages rendered in this frame.
    pub dirty_pages: u32,
    /// Requested pages denied by the physical budget.
    pub missing_pages: u32,
    /// Requested static pages reused without rendering.
    pub cache_hits: u32,
    /// Shadow clusters emitted by light-space selection/culling.
    pub clusters_drawn: u32,
    /// Dropped clusters; a nonzero value fails benchmark verification.
    pub cluster_overflow: u32,
    /// Physical depth pool bytes, excluding metadata.
    pub page_pool_bytes: u64,
}
