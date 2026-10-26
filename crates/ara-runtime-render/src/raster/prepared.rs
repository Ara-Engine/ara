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
    pub(super) native: ara_render::NativeContent,
}

impl RasterContent {
    /// Resolve published assets before calling this. Geometry conversion,
    /// instancing, material resolution and compressed mip registration run here;
    /// no device, submission, importer or shared mutable state is required.
    pub fn new(frame: RenderFrame, content: Content) -> Result<Self> {
        Self::prepare(frame, content, None)
    }

    /// Prepare on the content worker using the live renderer's small immutable
    /// reuse snapshot. Static vertex/index bytes stay on the GPU across generations.
    pub fn new_reusing(
        frame: RenderFrame,
        content: Content,
        seed: ara_render::NativeSeed,
    ) -> Result<Self> {
        Self::prepare(frame, content, Some(seed))
    }

    fn prepare(
        frame: RenderFrame,
        content: Content,
        seed: Option<ara_render::NativeSeed>,
    ) -> Result<Self> {
        ara_gpu::cpu_scope!("RasterContent.prepare_cpu");
        let mut prepared = native::prepare(&frame, content.clone(), texture_config())?;
        prepared.enable_motion();
        let key = content_key(&prepared);
        let textures = prepared.textures.fork_sources(texture_config())?;
        // NativeContent owns the immutable validation proof until publication;
        // restore this exact scene before any Prepared pose/material operation.
        let scene = std::mem::replace(
            &mut prepared.scene,
            ara_render::instanced::Scene {
                geometry: Vec::new(),
                instances: Vec::new(),
                materials: Vec::new(),
                textures: 0,
                lights: Vec::new(),
                sun_direction: glam30::Vec3::Y,
                sun_lux: glam30::Vec3::ZERO,
                environment: vec![[0., 0., 0., 1.]],
                environment_size: [1, 1],
            },
        );
        let native = ara_render::NativeContent::new(scene, Box::new(textures), seed)?;
        Ok(Self {
            frame,
            content,
            prepared,
            key,
            native,
        })
    }

    /// Poll native allocation/staging without submitting, waiting, or replacing
    /// the scene. Keep drawing last-good content while this returns false.
    pub fn preallocate(&mut self, renderer: &mut Renderer) -> Result<bool> {
        self.native.preallocate(renderer)
    }

    /// Cancel a superseded preparation and retire adopted resources without
    /// touching the renderer's currently published content.
    pub fn destroy(self, renderer: &mut Renderer) -> Result<()> {
        self.native.destroy(renderer)
    }
}
