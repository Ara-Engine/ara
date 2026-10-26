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
    /// Poll the retained preview without requesting another temporal frame.
    /// Use this after a settling burst so its final pending image is published.
    pub(in crate::app) fn poll(
        &mut self,
        renderer: &mut Renderer,
    ) -> ara_ui::Result<Option<ara_render::ViewOutput>> {
        self.view
            .as_ref()
            .map(|view| renderer.poll_view(view))
            .transpose()
            .map(Option::flatten)
            .map_err(Into::into)
    }

    /// A visible Game panel must keep polling while shared uploads/draws complete.
    pub(in crate::app) fn pending(&self, renderer: &Renderer) -> bool {
        self.view.is_some() && renderer.scene_frame_pending()
    }
    /// Release an inactive preview without waiting. A submitted frame retains
    /// its resources until completion; detach the UI image before this call.
    pub(in crate::app) fn release(&mut self, renderer: &mut Renderer) -> ara_ui::Result<()> {
        let pending = self.pending(renderer);
        if let Some(view) = self.view.take() {
            renderer.release_view(view)?;
        }
        debug_assert!(
            !pending || renderer.scene_frame_pending(),
            "releasing a preview must retain its pending completion"
        );
        Ok(())
    }
}

impl SceneView {
    /// Shared content work that a visible Game preview must poll. Scene-camera
    /// accumulation is independent and must not keep the Game panel awake.
    pub(in crate::app) fn preview_needs_frame(&self) -> bool {
        if self
            .recovery
            .blocked(self.content.as_ref().map_or(0, |c| c.revision()))
        {
            return false;
        }
        self.raster_gpu_pending
            || self.recovery.pending()
            || self.content_job.is_some()
            || self.content_ready.is_some()
            || self
                .raster
                .as_ref()
                .is_some_and(ara_runtime_render::RasterView::needs_frame)
            || self.content.as_ref().is_some_and(|content| {
                content.revision() != self.revision.map_or(0, |revision| revision.3)
                    || content.progress_pending()
            })
    }
    /// Poll the existing content owner, then render only the Game camera's extent.
    /// This never draws a Scene frame, changes its camera, or invokes capture.
    /// None means no preview has completed yet; the caller keeps its empty
    /// or loading presentation and polls again. Drive temporal frames on demand.
    pub(in crate::app) fn preview_game(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        document: &crate::world::DocumentView,
        preview: &mut GamePreview,
        request: PreviewCamera<'_>,
    ) -> ara_ui::Result<Option<ara_render::ViewOutput>> {
        ara_gpu::cpu_scope!("SceneView.preview_game");
        if request.size.contains(&0) {
            return Ok(None);
        }
        let _ = preview.poll(renderer)?;
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
            ara_runtime_render::environment(frame).manual_exposure(frame.environment.exposure)
        });
        if preview.view.is_none() {
            preview.view = Some(renderer.create_view()?);
        }
        let view = preview.view.as_mut().ok_or("Game preview camera")?;
        let request = ara_render::ViewRequest {
            camera: request.camera,
            size: request.size,
            render_size: plan.render_size,
            quality: plan.quality,
            exposure,
        };
        if !renderer.preallocate_view(view, &request)? {
            return renderer.view_output(view).map_err(Into::into);
        }
        let mut submitted = false;
        let output = renderer.submit_secondary(view, &request, |renderer, request| {
            // Content adoption and probe delivery use the same deferred upload
            // scope as this camera. Polling unchanged content must not extract
            // another snapshot or restart GI/emitter jobs.
            self.render_raster_inner(renderer, editor, document, RasterWork::Prepare)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let Some(frame) = &self.raster_frame else {
                return Ok(false);
            };
            if !renderer.is_native()
                || self.raster_scene.as_deref() != Some(editor.state.doc.as_str())
            {
                return Ok(false);
            }
            if self
                .raster
                .as_ref()
                .is_none_or(|r| !r.has_prepared_content())
            {
                return Ok(false);
            }
            request.exposure =
                ara_runtime_render::environment(frame).manual_exposure(frame.environment.exposure);
            self.raster
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Game preview content"))?
                .prepare_secondary_frame(renderer)?;
            submitted = true;
            Ok(true)
        })?;
        if submitted {
            self.raster_gpu_pending = renderer.probe_gi_needs_frame()
                || renderer
                    .native_texture_stats()
                    .is_some_and(|t| t.pending_textures > 0);
            if self.raster_gpu_pending {
                self.convergence.reset();
            }
            self.raster
                .as_mut()
                .ok_or("Game preview content")?
                .secondary_frame_submitted();
        }
        Ok(output)
    }
}
