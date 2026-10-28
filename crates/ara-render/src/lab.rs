//! Dev-only declarations. Passes can add typed keys and use their uniform slots in
//! compile-time experimental PSOs; shipping builds contain none of this module.
use anyhow::Result;
use ara_gpu::tunables::{Key, Registry};

/// Common look controls, applied over the scene's authored look.
#[derive(Clone, Copy)]
pub struct Keys {
    exposure: Key<f32>,
    curve: Key<i32>,
    bloom: Key<bool>,
    gain: Key<[f32; 4]>,
    saturation: Key<f32>,
    ao: Key<f32>,
    ssr: Key<f32>,
    manual: Key<bool>,
    ev: Key<f32>,
}
/// Plain values read by the host at a frame boundary, without allocation or locks.
#[derive(Clone, Copy)]
pub struct Values {
    /// Stops added to authored exposure compensation.
    pub exposure_stops: f32,
    /// -1 preserves the authored curve; 0 ACES fit, 1 neutral, 2 filmic.
    pub curve: i32,
    /// Whether authored bloom is allowed to run.
    pub bloom: bool,
    /// Multiplier on display gain; fourth component is reserved.
    pub gain: [f32; 4],
    /// Multiplier on authored colour saturation.
    pub saturation: f32,
    /// Multiplier on authored AO strength.
    pub ao: f32,
    /// Multiplier on authored reflection strength.
    pub ssr: f32,
    /// Force manual exposure for matched-transform scoring of an automatic scene.
    pub manual: bool,
    /// EV100 used when `manual` is true.
    pub ev100: f32,
}
/// Render setup declares types and bounds once. Other render modules may declare
/// additional keys through the same owned registry during their own setup.
pub fn declare(registry: &mut Registry) -> Result<()> {
    registry.declare("post.exposure_stops", 0_f32, -8., 8.)?;
    registry.declare("post.curve", -1_i32, -1., 2.)?;
    registry.declare("post.bloom_enabled", true, 0., 1.)?;
    registry.declare("post.gain_scale", [1_f32; 4], 0., 4.)?;
    registry.declare("post.saturation_scale", 1_f32, 0., 2.)?;
    registry.declare("ao.intensity_scale", 1_f32, 0., 4.)?;
    registry.declare("ssr.intensity_scale", 1_f32, 0., 1.)?;
    registry.declare("post.manual_exposure", false, 0., 1.)?;
    registry.declare("post.ev100", 10_f32, -10., 24.)?;
    Ok(())
}
impl Keys {
    /// Resolve names/types once; subsequent reads use direct typed slots.
    pub fn resolve(registry: &Registry) -> Result<Self> {
        Ok(Self {
            exposure: registry.key("post.exposure_stops")?,
            curve: registry.key("post.curve")?,
            bloom: registry.key("post.bloom_enabled")?,
            gain: registry.key("post.gain_scale")?,
            saturation: registry.key("post.saturation_scale")?,
            ao: registry.key("ao.intensity_scale")?,
            ssr: registry.key("ssr.intensity_scale")?,
            manual: registry.key("post.manual_exposure")?,
            ev: registry.key("post.ev100")?,
        })
    }
    /// Read a POD snapshot. The host's usual look upload carries these controls.
    pub fn read(self, registry: &Registry) -> Values {
        Values {
            exposure_stops: registry.get(self.exposure),
            curve: registry.get(self.curve),
            bloom: registry.get(self.bloom),
            gain: registry.get(self.gain),
            saturation: registry.get(self.saturation),
            ao: registry.get(self.ao),
            ssr: registry.get(self.ssr),
            manual: registry.get(self.manual),
            ev100: registry.get(self.ev),
        }
    }
}
