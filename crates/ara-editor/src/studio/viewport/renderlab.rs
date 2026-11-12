//! The Scene view's Renderlab frame. The Scene view draws the frame the editor extracts through
//! an engine hosted in the editor process (`ara-renderlab`'s bridge, the same one the player's
//! presenter uses), and the compositor shows that image. It is the only renderer: ids, picking
//! and outlines come from the frame's own CPU ids (`ids`) until Renderlab delivers its own.
use super::ids::{IdHost, Ids};
use anyhow::{Context, Result};
use ara_gpu::{GpuDevice, Image};
use ara_render_fidelity::environment::FrameEnvironment;
use ara_renderlab::{Options, PathChoice, Renderlab};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{
    ExternalRenderer, ExternalView, PresentationImage, fidelity::native::Content,
};
use std::path::Path;

/// The Scene content's revision (`SceneView::revision`).
pub(super) type Revision = Option<(u64, u64, u64, u64)>;

/// Distance in metres to which terrain layers use their detail maps: the High preset's.
const TERRAIN_DETAIL: f32 = 150.;

/// The editor-hosted Renderlab engine and the last image it drew.
pub(super) struct RenderlabScene {
    engine: Box<dyn ExternalRenderer>,
    /// What the frame's content resolves to, kept until the Scene's revision changes.
    content: Option<(Revision, Content)>,
    /// The completed image, a copy the compositor can sample while the next frame renders.
    published: Option<PresentationImage>,
    /// The Scene's object ids, rasterised from the frame beside the image: what clicks
    /// read, and what the overlays sample while the engine delivers none.
    ids: Ids,
    /// The engine's own packed ids (`ExternalFrame::id`), when it delivers them: a copy the
    /// compositor can sample while the next frame renders.
    published_ids: Option<PresentationImage>,
}

impl RenderlabScene {
    /// Starts the engine (hardware ray tracing where the GPU has it, else the non-RT path).
    /// `cache` is the project's GPU cache directory.
    pub(super) fn new(cache: &Path) -> Result<Self> {
        let renderlab = Renderlab::new(&Options {
            path: PathChoice::Auto,
            pipeline_cache: Some(cache.join("renderlab")),
            ..Default::default()
        })?;
        Ok(Self {
            engine: Box::new(renderlab),
            content: None,
            published: None,
            ids: Ids::default(),
            published_ids: None,
        })
    }

    /// Draws `frame` from `camera` at `size` and publishes the image. `revision` identifies the
    /// Scene's content: `resolve` (the environment map, foliage parts and terrain layer
    /// materials) runs only when it changes.
    pub(super) fn render(
        &mut self,
        gpu: &mut GpuDevice,
        frame: &RenderFrame,
        look: &FrameEnvironment,
        size: [u32; 2],
        revision: Revision,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        if self
            .content
            .as_ref()
            .is_none_or(|(key, _)| *key != revision)
        {
            self.content = Some((revision, resolve()?));
        }
        let (_, content) = self.content.as_ref().context("content resolved above")?;
        let rendered = self.engine.render(
            gpu,
            &ExternalView {
                frame,
                size,
                look,
                environment: content.environment.as_ref(),
                foliage_parts: &content.foliage_parts,
                terrain_materials: &content.terrain_materials,
                debug_switches: false,
                foliage_percent: 100,
                terrain_detail: TERRAIN_DETAIL,
            },
        )?;
        PresentationImage::publish(&mut self.published, gpu, rendered.image, size)?;
        if let Some(id) = rendered.id {
            PresentationImage::publish(&mut self.published_ids, gpu, id, size)?;
        }
        self.ids.update(gpu, frame, size)?;
        Ok(())
    }

    /// The packed id image for the same frame as [`Self::image`] (`IdSource::Packed`).
    pub(super) fn id_image(&self) -> Option<Image> {
        self.published_ids
            .as_ref()
            .map(PresentationImage::output)
            .or_else(|| self.ids.image())
    }

    /// The ids' channels and texels, for clicks, hover and the channel map.
    pub(super) fn id_host(&self) -> Option<IdHost<'_>> {
        self.ids.host()
    }

    /// The last completed image, for the compositor.
    pub(super) fn image(&self) -> Option<Image> {
        self.published.as_ref().map(PresentationImage::output)
    }

    /// Releases what the engine and the published image hold on `gpu`.
    pub(super) fn release(mut self, gpu: &mut GpuDevice) -> Result<()> {
        if let Some(image) = self.published.take() {
            image.destroy(gpu)?;
        }
        if let Some(image) = self.published_ids.take() {
            image.destroy(gpu)?;
        }
        self.ids.release(gpu)?;
        self.engine.release(gpu)
    }
}
