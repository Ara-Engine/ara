//! Game-camera preview over the Scene's published content, never a capture route.
use super::*;

/// Lazy owned camera resources. The Game panel owns when to request frames and
/// keeps this handle alive while its external image is attached.
#[derive(Default)]
pub(in crate::app) struct GamePreview {
    view: Option<ara_render::ViewId>,
}

/// Camera, physical panel extent and raster quality selected by the Game panel.
pub(in crate::app) struct PreviewCamera<'a> {
    /// Gameplay camera, independent of the editor Scene camera.
    pub camera: &'a ara_render::scene::CameraSpec,
    /// Physical Game-panel pixels; zero extent skips work.
    pub size: [u32; 2],
    /// User choice, or None to inherit the project's rendering setting.
    pub quality: Option<ara_runtime_render::QualityChoice>,
}

impl GamePreview {
    /// Release an inactive preview immediately at a completed GPU boundary.
    /// Ordinary Drop also schedules retirement on the shared renderer.
    pub(in crate::app) fn release(&mut self, renderer: &mut Renderer) -> ara_ui::Result<()> {
        if let Some(view) = self.view.take() {
            renderer.release_view(view)?;
        }
        Ok(())
    }
}

impl SceneView {
    /// Poll the existing content owner, then render only the Game camera's extent.
    /// This never draws a Scene frame, changes its camera, or invokes capture.
    /// None means no content has been published yet; the caller keeps its empty
    /// or loading presentation and polls again. Drive temporal frames on demand.
    pub(in crate::app) fn preview_game(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        document: &crate::world::DocumentView,
        preview: &mut GamePreview,
        request: PreviewCamera<'_>,
    ) -> ara_ui::Result<Option<ara_render::ViewOutput>> {
        if request.size.contains(&0) {
            return Ok(None);
        }
        self.render_raster_inner(renderer, editor, document, RasterWork::Prepare)?;
        if renderer.scene_frame_pending() {
            return Ok(None);
        }
        if self.raster_frame.is_none() || !renderer.is_native() {
            return Ok(None);
        }
        let settings = crate::world::gltf::render_settings(editor.host.store().workspace())?;
        let plan = Plan::resolve(
            request.quality.unwrap_or(settings.quality.into()),
            &Capabilities::of(renderer.gpu()),
            request.size,
            ara_runtime_render::RenderOptions { ray_tracing: false },
        )?;
        let RenderPath::Raster(plan) = plan.path else {
            return Err("Game preview needs a raster plan".into());
        };
        let exposure = self.raster_frame.as_ref().map_or(1., |frame| {
            FrameEnvironment::resolve(&frame.look, frame.environment.exposure)
                .manual_exposure(frame.environment.exposure)
        });
        if preview.view.is_none() {
            preview.view = Some(renderer.create_view()?);
        }
        let view = preview.view.as_mut().ok_or("Game preview camera")?;
        let request = ara_render::ViewRequest {
            camera: request.camera,
            size: request.size,
            quality: plan.quality,
            exposure,
        };
        let output = self
            .raster
            .as_mut()
            .ok_or("Game preview native content")?
            .render_secondary(renderer, view, &request)?;
        Ok(Some(output))
    }
}
