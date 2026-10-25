//! One completion boundary for the synchronous player's view, particles and post.
use super::*;

impl RasterView {
    pub(crate) fn render_batched(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
    ) -> Result<GpuFrameCosts> {
        renderer
            .gpu_mut()
            .begin_deferred_completion_on_current_queue()?;
        let mut boundaries = [0; 2];
        let rendered = self.render_recorded(renderer, frame, camera, mode, &mut boundaries);
        // Even a rejected frame may have submitted uploads/draws. Drain those
        // before another frame reuses storage; publication belongs to the caller
        // and is reached only after this whole operation succeeds.
        let mut completion = renderer.gpu_mut().end_deferred_completion()?;
        let mut view = Vec::new();
        let mut effects_ms = 0.;
        let mut index = 0;
        renderer
            .gpu_mut()
            .wait_completion_with(&mut completion, |_, passes| {
                if (boundaries[0]..boundaries[1]).contains(&index) {
                    effects_ms += passes.iter().map(|pass| pass.milliseconds).sum::<f64>();
                } else {
                    view.extend(passes);
                }
                index += 1;
            })?;
        let mut costs = rendered?;
        costs.raster_ms = view.iter().map(|pass| pass.milliseconds).sum();
        costs.effects_ms = effects_ms;
        renderer.set_completed_timings(view);
        Ok(costs)
    }
}
