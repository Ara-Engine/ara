//! Controls use typed registry slots; only a control update queues a uniform upload.
use super::*;
use ara_gpu::tunables;
use ara_render_fidelity::{colour::ToneMapper, environment::FrameEnvironment};
use std::collections::BTreeMap;

impl State {
    pub fn set_params(&mut self, presenter: &mut Presenter, value: Value) -> Result<Value> {
        let changes: BTreeMap<String, tunables::Value> = serde_json::from_value(value)?;
        let gpu = presenter
            .renderer_mut()
            .context("warm renderer is not ready")?
            .gpu_mut();
        gpu.lab_params_mut().set(&changes)?;
        gpu.flush_lab_params()?;
        self.keys = Some(ara_render::lab::Keys::resolve(gpu.lab_params())?);
        Ok(gpu.lab_params().report())
    }
    pub fn look(&self, presenter: &Presenter, frame: &RenderFrame) -> FrameEnvironment {
        let mut look = ara_runtime_render::environment(frame);
        if let Some(keys) = self.keys
            && let Some(renderer) = presenter.renderer()
        {
            let values = keys.read(renderer.gpu().lab_params());
            look.compensation += values.exposure_stops;
            if values.manual {
                look.ev100 = Some(values.ev100);
            }
            look.tone_mapper = match values.curve {
                0 => ToneMapper::AcesFit,
                1 => ToneMapper::PbrNeutral,
                2 => ToneMapper::Filmic,
                _ => look.tone_mapper,
            };
            if !values.bloom {
                look.bloom = 0.;
            }
            for (gain, scale) in look.display[2].iter_mut().zip(values.gain) {
                *gain *= scale;
            }
            look.grading.saturation = (look.grading.saturation * values.saturation).clamp(0., 2.);
            look.ao.intensity = (look.ao.intensity * values.ao).clamp(0., 4.);
            look.ssr.intensity = (look.ssr.intensity * values.ssr).clamp(0., 1.);
        }
        look
    }
}
impl Window {
    pub fn apply_params(&self, presenter: &mut Presenter, frame: &RenderFrame) {
        if self.options.lab_idle && self.lab.keys.is_some() {
            let look = self.lab.look(presenter, frame);
            presenter.set_look_override(Some(look));
        }
    }
}
