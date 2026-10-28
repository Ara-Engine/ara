//! One completion boundary for the player's view, particles, post and publication.
use super::*;
use ara_gpu::vulkan::PassTiming;

/// A raster frame submitted in one deferred scope. [`RasterView::finish_batched`]
/// waits it: at once on synchronous presenters, or after the next frame is
/// submitted with frames in flight (`Presenter::set_frames_in_flight`).
pub(crate) struct Submitted {
    /// Optional recorder identity of this submission, not its readback frame.
    pub profile: Option<u64>,
    completion: ara_gpu::GpuCompletion,
    /// Effects submissions, in scope order (the rest is the view).
    boundaries: [usize; 2],
    /// First submission of the in-scope publication; later ones are publication.
    publication: usize,
    rendered: Result<GpuFrameCosts>,
}

/// An open frame scope (`RasterView::begin_batched`). `prepared` is the result
/// of work the owner did inside it before recording (preparation); on error the
/// frame is not recorded, but the scope still ends with a completion to drain.
pub(crate) struct Opened {
    completed: Vec<PassTiming>,
    pub prepared: Result<()>,
}

/// A finished frame: its costs, and its own device span and submission gaps in
/// milliseconds (exact while another frame overlaps it in flight).
pub(crate) struct Finished {
    /// Recorder identity carried intact through bounded completion retries.
    pub profile: Option<u64>,
    pub costs: GpuFrameCosts,
    pub span_ms: Option<f64>,
    pub gaps_ms: Option<f64>,
}

impl RasterView {
    /// Opens the frame's deferred scope. Recording clears the renderer's timings;
    /// the returned last completed ones stay visible until this frame completes.
    pub(crate) fn begin_batched(renderer: &mut Renderer) -> Result<Opened> {
        renderer
            .gpu_mut()
            .begin_deferred_completion_on_current_queue()?;
        Ok(Opened {
            completed: renderer.take_timings(),
            prepared: Ok(()),
        })
    }

    /// Records the frame (unless the scope's `prepared` work failed) and `publish`
    /// (output publication), then ends the scope; nothing waits. Even a
    /// rejected frame may have submitted uploads/draws, so the result always carries
    /// the completion to drain before another frame reuses storage; errors surface
    /// from `finish_batched`.
    pub(crate) fn end_batched(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
        opened: Opened,
        publish: impl FnOnce(&mut Renderer) -> Result<()>,
    ) -> Result<Submitted> {
        let Opened {
            completed,
            prepared,
        } = opened;
        let mut boundaries = [0; 2];
        let mut rendered = prepared
            .and_then(|()| self.render_recorded(renderer, frame, camera, mode, &mut boundaries));
        let publication = renderer.gpu().deferred_submission_count();
        if rendered.is_ok()
            && let Err(error) = publish(renderer)
        {
            rendered = Err(error);
        }
        let completion = renderer.gpu_mut().end_deferred_completion()?;
        renderer.set_completed_timings(completed);
        Ok(Submitted {
            profile: None,
            completion,
            boundaries,
            publication,
            rendered,
        })
    }

    /// Opens a scope, records and submits the frame, then `publish` in the same
    /// scope (`begin_batched` + `end_batched`).
    pub(crate) fn submit_batched(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
        publish: impl FnOnce(&mut Renderer) -> Result<()>,
    ) -> Result<Submitted> {
        let opened = Self::begin_batched(renderer)?;
        self.end_batched(renderer, frame, camera, mode, opened, publish)
    }

    /// Waits a [`Submitted`] frame and splits its pass timings into view, effects
    /// and publication; the view's become the renderer's completed timings.
    pub(crate) fn finish_batched(
        renderer: &mut Renderer,
        submitted: Submitted,
    ) -> Result<Finished> {
        match Self::finish_within(renderer, submitted, None)? {
            Ok(finished) => Ok(finished),
            Err(_) => Err(anyhow::anyhow!("an unbounded frame wait returned early")),
        }
    }

    /// `finish_batched`, waiting at most `timeout` (None: until done). Err returns the
    /// still-pending frame, unconsumed, for a later retry: a stalled present path (a
    /// minimized or undisplayed window) must not block the caller's event pump.
    pub(crate) fn finish_within(
        renderer: &mut Renderer,
        mut submitted: Submitted,
        timeout: Option<std::time::Duration>,
    ) -> Result<std::result::Result<Finished, Submitted>> {
        let (boundaries, publication) = (submitted.boundaries, submitted.publication);
        let mut view = Vec::new();
        let (mut effects_ms, mut publication_ms) = (0., 0.);
        let mut index = 0;
        let mut consume = |_, passes: Vec<PassTiming>| {
            let ms = passes.iter().map(|pass| pass.milliseconds).sum::<f64>();
            if index >= publication {
                publication_ms += ms;
            } else if (boundaries[0]..boundaries[1]).contains(&index) {
                effects_ms += ms;
            } else {
                view.extend(passes);
            }
            index += 1;
        };
        let gpu = renderer.gpu_mut();
        let done = match timeout {
            None => {
                gpu.wait_completion_with(&mut submitted.completion, &mut consume)?;
                true
            }
            Some(timeout) => {
                gpu.wait_completion_for(&mut submitted.completion, timeout, &mut consume)?
            }
        };
        if !done {
            return Ok(Err(submitted));
        }
        let Submitted {
            completion,
            rendered,
            profile,
            ..
        } = submitted;
        let mut costs = rendered?;
        costs.raster_ms = view.iter().map(|pass| pass.milliseconds).sum();
        costs.effects_ms = effects_ms;
        costs.publication_ms = publication_ms;
        renderer.set_completed_timings(view);
        Ok(Ok(Finished {
            profile,
            costs,
            span_ms: completion.timing_window().map(|span| span.milliseconds()),
            gaps_ms: completion.timing_window_gaps(),
        }))
    }
}
