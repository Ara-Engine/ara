//! Bind the shared workload contract to completed presentation frames, never PSO guesses.
use super::*;
use ara_bench::passes::{Expected, Frame, Guard, PassError};

pub(super) fn frame(presenter: &Presenter) -> Option<Frame> {
    if presenter.stats().skipped {
        return None;
    }
    presenter
        .stats()
        .gpu
        .map(|_| Frame::from_names(presenter.pass_timings().iter().map(|p| p.pass.as_str())))
}
impl Window {
    pub fn configure_rt(&mut self, fidelity_rt: bool, quality: ara_runtime_render::QualityChoice) {
        self.fidelity_rt = fidelity_rt;
        self.requested_preset = match quality {
            ara_runtime_render::QualityChoice::Auto => None,
            ara_runtime_render::QualityChoice::Preset(preset) => Some(preset),
        };
    }
    pub(super) fn passes_ready(
        &mut self,
        presenter: &Presenter,
        frame: Option<Frame>,
    ) -> Result<bool, PassError> {
        if self.pass_guard.is_none() {
            let (Some(preset), Some(renderer)) = (presenter.quality(), presenter.renderer()) else {
                return Ok(false);
            };
            self.pass_guard = Some(Guard::configured(
                Expected::resolve(
                    self.requested_preset.unwrap_or(preset).name(),
                    renderer.gpu().info().ray_tracing,
                    self.fidelity_rt,
                ),
                Duration::from_millis(self.options.perf_ready_timeout_ms),
            )?);
        }
        if self.measured > 0 {
            return Ok(frame.is_some());
        }
        match &mut self.pass_guard {
            Some(guard) => guard.wait(
                self.began.elapsed(),
                if self
                    .requested_preset
                    .is_some_and(|expected| presenter.quality() != Some(expected))
                {
                    None
                } else {
                    frame
                },
            ),
            None => Ok(false),
        }
    }
}
