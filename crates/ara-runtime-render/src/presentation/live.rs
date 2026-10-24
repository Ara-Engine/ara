//! Temporal native rendering and persistent VFX.
use super::*;

impl Live {
    pub(super) fn render(
        &mut self,
        gpu: &mut VulkanDevice,
        frame: &RenderFrame,
    ) -> Result<GpuFrameCosts> {
        let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
        if self.look != look || self.tick.is_none() {
            self.view.set_environment(&look)?;
            self.look = look;
        }
        let textures = self.view.refine_native_textures(gpu)?;
        let camera = camera(frame)?;
        self.temporal = self.temporal.wrapping_add(1);
        // Particle frames get their bloom/post/upscale from the composite below.
        self.view.set_deferred_post(!frame.effects.is_empty());
        self.view.render(
            gpu,
            &FrameInput {
                camera: &camera,
                tick: self.temporal,
                delta_seconds: 1. / 60.,
                reset: self.tick.is_none(),
                lighting_changed: std::mem::take(&mut self.lighting_changed),
                deterministic: false,
            },
        )?;
        self.effects.update(
            gpu,
            &frame.effects,
            (
                self.prepared.scene.sun_direction,
                look.ground_sun(
                    self.prepared.scene.sun_direction,
                    self.prepared.scene.sun_lux,
                ),
            ),
        )?;
        if !self.effects.is_empty() {
            let ticks = self
                .tick
                .map_or(0, |old| frame.tick.saturating_sub(old).min(240) as u32);
            self.effects
                .composite_ticks(gpu, &mut self.view, &camera, self.temporal, ticks)?;
        }
        self.tick = Some(frame.tick);
        Ok(GpuFrameCosts {
            texture_upload_ms: textures.upload_gpu_ms,
            texture_uploaded_bytes: textures.uploaded_bytes,
            texture_pending: textures.pending_textures,
            texture_resident_bytes: textures.resident_bytes,
            fidelity_ms: self
                .view
                .timings()
                .iter()
                .map(|pass| pass.milliseconds)
                .sum(),
            effects_ms: if self.effects.is_empty() {
                0.
            } else {
                self.effects.gpu_ms()
            },
            ..Default::default()
        })
    }
    pub(super) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        self.effects.destroy(gpu)?;
        self.view.destroy(gpu)
    }
}
