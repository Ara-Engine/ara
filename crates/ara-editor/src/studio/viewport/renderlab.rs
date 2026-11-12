//! The Scene view's Renderlab frame. The Scene view draws the frame the editor extracts through
//! an engine hosted in the editor process (`ara-renderlab`'s bridge, the same one the player's
//! presenter uses), and the compositor shows that image. It is the only renderer: ids, picking
//! and outlines come from the frame's own CPU ids (`ids`) until Renderlab delivers its own.
use super::ids::{IdHost, Ids};
use anyhow::{Context, Result};
use ara_gpu::{GpuDevice, Image, TimelinePoint};
use ara_render_fidelity::environment::FrameEnvironment;
use ara_renderlab::{Options, PathChoice, Renderlab};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{
    ExternalFrame, ExternalRenderer, ExternalView, PresentationImage, fidelity::native::Content,
};
use std::path::Path;

/// The Scene content's revision (`SceneView::revision`).
pub(super) type Revision = Option<(u64, u64, u64, u64)>;

/// Distance in metres to which terrain layers use their detail maps: the High preset's.
const TERRAIN_DETAIL: f32 = 150.;

/// A frame submitted and not yet taken: what the overlays and the ids need to agree with it.
struct Flight {
    /// The editor's camera at submission.
    camera: crate::state::Camera,
    size: [u32; 2],
    /// The frame's channels (`ids::entities`): draw `n` is id `n + 1`.
    entities: Vec<(u64, String)>,
}

impl Flight {
    /// Whether the frame still fits the Scene view: drawn at the size it has now.
    fn is_current(&self, size: [u32; 2]) -> bool {
        self.size == size
    }
}

/// The frame on screen.
struct Shown {
    camera: crate::state::Camera,
    size: [u32; 2],
}

/// The editor-hosted Renderlab engine and the last image it drew.
pub(super) struct RenderlabScene {
    engine: Box<dyn ExternalRenderer>,
    /// What the frame's content resolves to, kept until the Scene's revision changes.
    content: Option<(Revision, Content)>,
    /// The completed image, a copy the compositor can sample while the next frame renders.
    published: Option<PresentationImage>,
    /// The Scene's channels and, when the engine hands its ids across as CPU texels, those
    /// texels (what a click falls back to and a capture reads) and their GPU copy for the overlays.
    ids: Ids,
    /// The engine's own packed ids (`ExternalFrame::id`), when it delivers them: a copy the
    /// compositor can sample while the next frame renders.
    published_ids: Option<PresentationImage>,
    /// The frame submitted and not yet taken (`start`, `poll`).
    flight: Option<Flight>,
    /// The frame whose image is on screen.
    shown: Option<Shown>,
}

impl RenderlabScene {
    /// Starts the engine (hardware ray tracing where the GPU has it, else the non-RT path).
    /// `cache` is the project's GPU cache directory.
    pub(super) fn new(cache: &Path) -> Result<Self> {
        let renderlab = Renderlab::new(&Options {
            path: PathChoice::Auto,
            pipeline_cache: Some(cache.join("renderlab")),
            scene_ids: true,
            ..Default::default()
        })?;
        Ok(Self {
            engine: Box::new(renderlab),
            content: None,
            published: None,
            ids: Ids::default(),
            published_ids: None,
            flight: None,
            shown: None,
        })
    }

    /// Draws `frame` seen by `camera` at `size` and publishes the image once it is done. With an
    /// engine that splits a frame (everywhere but Apple) this only submits it: it returns at
    /// once, and [`Self::poll`] takes the result; one frame is in flight at a time. `revision`
    /// identifies the Scene's content: `resolve` (the environment map, foliage parts and terrain
    /// layer materials) runs only when it changes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start(
        &mut self,
        gpu: &mut GpuDevice,
        frame: &RenderFrame,
        look: &FrameEnvironment,
        size: [u32; 2],
        revision: Revision,
        camera: crate::state::Camera,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        anyhow::ensure!(self.flight.is_none(), "a Scene frame is already in flight");
        if self
            .content
            .as_ref()
            .is_none_or(|(key, _)| *key != revision)
        {
            self.content = Some((revision, resolve()?));
        }
        let (_, content) = self.content.as_ref().context("content resolved above")?;
        let view = ExternalView {
            frame,
            size,
            look,
            environment: content.environment.as_ref(),
            foliage_parts: &content.foliage_parts,
            terrain_materials: &content.terrain_materials,
            debug_switches: false,
            foliage_percent: 100,
            terrain_detail: TERRAIN_DETAIL,
        };
        // The ids belong to the frame as submitted, whatever the editor extracts meanwhile.
        let flight = Flight {
            camera,
            size,
            entities: super::ids::entities(frame),
        };
        if self.engine.begin(gpu, &view)? {
            self.flight = Some(flight);
            return Ok(());
        }
        let rendered = self.engine.render(gpu, &view)?;
        self.adopt(gpu, rendered, flight)
    }

    /// Takes the frame [`Self::start`] submitted once the GPU has finished it, and publishes its
    /// image and ids; whether one arrived. Never waits for the GPU.
    pub(super) fn poll(&mut self, gpu: &mut GpuDevice, size: [u32; 2]) -> Result<bool> {
        if self.flight.is_none() {
            return Ok(false);
        }
        let Some(rendered) = self.engine.finish(gpu)? else {
            return Ok(false);
        };
        let flight = self.flight.take().context("a flight was checked above")?;
        // A frame of the old size (the view was resized while it drew) is never shown: its image
        // and ids belong to the old rectangle. The next frame carries the new size.
        if !flight.is_current(size) {
            return Ok(false);
        }
        self.adopt(gpu, rendered, flight)?;
        Ok(true)
    }

    /// The size of the frame on screen.
    pub(super) fn shown_size(&self) -> Option<[u32; 2]> {
        self.shown.as_ref().map(|shown| shown.size)
    }

    /// Whether a frame was submitted and not yet taken.
    pub(super) fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    /// The camera the image on screen was drawn from: the overlays draw from it, so the grid,
    /// gizmos and icons agree with the picture whatever the live camera has done since.
    pub(super) fn shown_camera(&self) -> Option<crate::state::Camera> {
        self.shown.as_ref().map(|shown| shown.camera)
    }

    /// Publishes a finished frame: its image, its ids and the camera it was drawn from. A frame
    /// of another size than the one asked for since (a resize) is dropped.
    fn adopt(
        &mut self,
        gpu: &mut GpuDevice,
        rendered: ExternalFrame,
        flight: Flight,
    ) -> Result<()> {
        PresentationImage::publish(&mut self.published, gpu, rendered.image, flight.size)?;
        if let Some(id) = rendered.id {
            PresentationImage::publish(&mut self.published_ids, gpu, id, flight.size)?;
        }
        self.ids
            .update(gpu, flight.entities, flight.size, rendered.id_texels)?;
        self.shown = Some(Shown {
            camera: flight.camera,
            size: flight.size,
        });
        Ok(())
    }

    /// The engine's id under `pixel` for a click (frontmost, glass included), when it has one.
    pub(super) fn pick_id(&mut self, pixel: [u32; 2]) -> Option<u32> {
        match self.engine.pick_id(pixel) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("ARA_SCENE_RENDERER: Renderlab could not answer the click ({error:#})");
                None
            }
        }
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

    /// The latest submission that wrote what the compositor samples (the image and the ids
    /// copies, all on the graphics queue): the UI orders after this point alone, so Renderlab's
    /// next frame can draw while the UI composes this one.
    pub(super) fn published_point(&self) -> Option<TimelinePoint> {
        [
            self.published.as_ref().and_then(PresentationImage::point),
            self.published_ids
                .as_ref()
                .and_then(PresentationImage::point),
            self.ids.point(),
        ]
        .into_iter()
        .flatten()
        .max_by_key(|point| point.value)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that was in flight when the view resized is not current: it is dropped, never
    /// presented into the new rectangle with the old ids.
    #[test]
    fn a_frame_drawn_at_the_old_size_is_not_current() {
        let flight = Flight {
            camera: crate::state::Camera::default(),
            size: [906, 574],
            entities: Vec::new(),
        };
        assert!(flight.is_current([906, 574]));
        assert!(!flight.is_current([1200, 574]));
        assert!(!flight.is_current([906, 700]));
    }
}
