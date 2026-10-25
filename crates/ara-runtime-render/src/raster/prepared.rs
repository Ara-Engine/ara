//! Owned CPU preparation, separate from frame-thread GPU publication.
use super::*;

/// A complete immutable extraction and its CPU-prepared native content. It owns
/// no GPU resources and can be built on a worker while the last scene is drawn.
/// Keeping the frame private prevents publishing geometry for a different frame.
pub struct RasterContent {
    pub(super) frame: RenderFrame,
    pub(super) content: Content,
    pub(super) prepared: Prepared,
    pub(super) key: u64,
}

impl RasterContent {
    /// Resolve published assets before calling this. Geometry conversion,
    /// instancing, material resolution and compressed mip registration run here;
    /// no device, submission, importer or shared mutable state is required.
    pub fn new(frame: RenderFrame, content: Content) -> Result<Self> {
        ara_gpu::cpu_scope!("RasterContent.prepare_cpu");
        let mut prepared = native::prepare(&frame, content.clone(), texture_config())?;
        prepared.enable_motion();
        let key = content_key(&prepared);
        Ok(Self {
            frame,
            content,
            prepared,
            key,
        })
    }
}
