//! Owned CPU preparation, separate from frame-thread GPU publication.
use super::*;

/// A complete immutable extraction and its CPU-prepared native content. It owns
/// no GPU resources initially and can be built on a worker while the last scene is drawn.
/// Keeping the frame private prevents publishing geometry for a different frame.
/// After `preallocate`, publish it or call `destroy` to cancel/retire its resources.
#[must_use = "publish prepared content or destroy its native preparation"]
pub struct RasterContent {
    pub(super) frame: RenderFrame,
    pub(super) content: Content,
    pub(super) prepared: Prepared,
    pub(super) key: u64,
    pub(super) textures: ara_texture_stream::NativeResidency,
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
        let textures = prepared.textures.fork_sources(texture_config())?;
        Ok(Self {
            frame,
            content,
            prepared,
            key,
            textures,
        })
    }

    /// Poll native allocation/staging without submitting, waiting, or replacing
    /// the scene. Keep drawing last-good content while this returns false.
    pub fn preallocate(&mut self, renderer: &mut Renderer) -> Result<bool> {
        renderer.preallocate_native_textures(&mut self.textures)
    }

    /// Cancel a superseded preparation and retire adopted resources without
    /// touching the renderer's currently published content.
    pub fn destroy(self, renderer: &mut Renderer) -> Result<()> {
        self.textures.destroy(renderer.gpu_mut())
    }
}
