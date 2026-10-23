//! Completed-frame sun visibility from a virtual shadow-map producer.
use crate::{FidelityView, FrameInput, ImageInput};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Queue};

/// Primary-surface sun shadows. Secondary transport and local lights retain ray
/// visibility, because a screen-sized VSM mask does not describe their receivers.
#[derive(Clone, Copy, Default)]
pub enum SunShadowSource {
    /// Trace the shared scene; hardware queries on RT, software on fallback tiers.
    #[default]
    RayQueries,
    /// Borrow the geometry/VSM producer's completed RGBA32F visibility image.
    VirtualShadowMap {
        /// Linear RGB sun visibility in [0,1]; A>=0.5 means resident and valid.
        /// Invalid pixels fall back to scene rays. The producer retains ownership.
        visibility: ImageInput,
        /// Must match this view's render size, camera and projection jitter.
        render_size: [u32; 2],
    },
}
impl FidelityView {
    /// Publish a completed Graphics/ShaderRead mask before the next render.
    /// Keep it alive through render completion; republish after a view resize.
    /// Lighting/camera edits still use FrameInput's normal reset/change flags.
    pub fn set_sun_shadow_source(&mut self, source: SunShadowSource) -> Result<()> {
        if let SunShadowSource::VirtualShadowMap {
            visibility,
            render_size,
        } = source
        {
            ensure!(
                render_size == self.settings.render_size
                    && visibility.state.queue == Queue::Graphics
                    && visibility.state.access == Access::ShaderRead,
                "VSM mask must match the completed view and be Graphics/ShaderRead"
            );
        }
        if matches!(source, SunShadowSource::RayQueries)
            != matches!(self.shadow_source, SunShadowSource::RayQueries)
        {
            self.history = false;
        }
        self.shadow_source = source;
        Ok(())
    }
    /// Jitter for an upcoming frame, in render pixels. External visibility and VSM
    /// producers use this before publishing a mask for that exact frame/camera.
    pub fn render_jitter(&self, input: &FrameInput<'_>) -> Result<[f32; 2]> {
        let (frame, _, _) = self.frame(input)?;
        Ok([frame.jitter[0], frame.jitter[1]])
    }
}
