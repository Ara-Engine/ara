//! One completion boundary for the player's view, particles, post and publication.
use super::*;

/// A raster frame submitted in one deferred scope. [`RasterView::finish_batched`]
/// waits it: at once on synchronous presenters, or at the start of the next frame
/// with frames in flight (`Presenter::set_frames_in_flight`).
pub(crate) struct Submitted {
    completion: ara_gpu::GpuCompletion,
    /// Effects submissions, in scope order (the rest is the view).
    boundaries: [usize; 2],
    /// First submission of the in-scope publication; later ones are publication.
    publication: usize,
    rendered: Result<GpuFrameCosts>,
}

impl RasterView {
    /// Records and submits the frame, then `publish` (output publication) in the
    /// same scope; nothing waits. Even a rejected frame may have submitted
    /// uploads/draws, so the result always carries the completion to drain before
    /// another frame reuses storage; errors surface from `finish_batched`.
    pub(crate) fn submit_batched(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
        publish: impl FnOnce(&mut Renderer) -> Result<()>,
    ) -> Result<Submitted> {
        renderer
            .gpu_mut()
            .begin_deferred_completion_on_current_queue()?;
        // Recording clears the renderer's timings; the last completed frame's stay
        // visible until this one completes (`finish_batched`).
        let completed = renderer.take_timings();
        let mut boundaries = [0; 2];
        let mut rendered = self.render_recorded(renderer, frame, camera, mode, &mut boundaries);
        let publication = renderer.gpu().deferred_submission_count();
        if rendered.is_ok()
            && let Err(error) = publish(renderer)
        {
            rendered = Err(error);
        }
        let completion = renderer.gpu_mut().end_deferred_completion()?;
        renderer.set_completed_timings(completed);
        Ok(Submitted {
            completion,
            boundaries,
            publication,
            rendered,
        })
    }

    /// Waits a [`Submitted`] frame and splits its pass timings into view, effects
    /// and publication; the view's become the renderer's completed timings.
    pub(crate) fn finish_batched(
        renderer: &mut Renderer,
        submitted: Submitted,
    ) -> Result<GpuFrameCosts> {
        let Submitted {
            mut completion,
            boundaries,
            publication,
            rendered,
        } = submitted;
        let mut view = Vec::new();
        let (mut effects_ms, mut publication_ms) = (0., 0.);
        let mut index = 0;
        renderer
            .gpu_mut()
            .wait_completion_with(&mut completion, |_, passes| {
                let ms = passes.iter().map(|pass| pass.milliseconds).sum::<f64>();
                if index >= publication {
                    publication_ms += ms;
                } else if (boundaries[0]..boundaries[1]).contains(&index) {
                    effects_ms += ms;
                } else {
                    view.extend(passes);
                }
                index += 1;
            })?;
        let mut costs = rendered?;
        costs.raster_ms = view.iter().map(|pass| pass.milliseconds).sum();
        costs.effects_ms = effects_ms;
        costs.publication_ms = publication_ms;
        renderer.set_completed_timings(view);
        Ok(costs)
    }
}
