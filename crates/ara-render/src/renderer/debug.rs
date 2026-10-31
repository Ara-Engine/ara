//! Diagnostic switches that remove one effect at a time, so its contribution to an
//! artifact (flicker, aliasing, darkening) can be seen in isolation. The editor's
//! View > Rendering debug menu sets them; shipping games never do.
use serde::{Deserialize, Serialize};

/// Effects a debug view may turn off. The default keeps everything on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderDebug {
    /// Screen-space ambient occlusion.
    pub ambient_occlusion: bool,
    /// Sun shadows (cascades, far map, virtual shadow maps, terrain horizon) and
    /// local-light shadows.
    pub shadows: bool,
    /// The temporal sub-pixel jitter that temporal anti-aliasing and upscaling
    /// accumulate; off, edges alias but hold still.
    pub temporal_jitter: bool,
    /// Render at display resolution instead of the preset's upscaled render scale.
    /// Applied where the render size is chosen (`ara-runtime-render`'s presenter).
    pub full_resolution: bool,
    /// What the lit image shows of reflections.
    pub reflections: ReflectionView,
    /// The debug view presenters draw instead of Lit (live windows and captures):
    /// `ara capture --view` and `play.renderDebug`.
    #[serde(default = "lit")]
    pub view: crate::capture::ViewMode,
}

fn lit() -> crate::capture::ViewMode {
    crate::capture::ViewMode::Lit
}

/// The reflection debug views: the lit image, the reflected light alone, or where
/// each pixel's reflection comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReflectionView {
    /// The lit image.
    #[default]
    Lit,
    /// Only the reflected (indirect specular) light, exposed like the lit image.
    Reflections,
    /// Each pixel coloured by the share of its reflection each source resolved:
    /// on-screen trace green, capture trace blue, box-projected captures cyan,
    /// probes orange, sky magenta; surfaces too rough to trace dimmed.
    Sources,
}

impl Default for RenderDebug {
    fn default() -> Self {
        Self {
            ambient_occlusion: true,
            shadows: true,
            temporal_jitter: true,
            full_resolution: false,
            reflections: ReflectionView::Lit,
            view: crate::capture::ViewMode::Lit,
        }
    }
}

impl RenderDebug {
    /// `quality` without the effects this turns off. Cascades keep rendering (their
    /// maps stay allocated) but the frame samples none of them (`Frame::look.z`).
    pub(super) fn apply(self, mut quality: crate::look::Quality) -> crate::look::Quality {
        if !self.ambient_occlusion {
            quality.ambient_occlusion = None;
        }
        if !self.shadows {
            quality.shadows.far = None;
            quality.local_shadows = None;
            quality.virtual_shadows = None;
            quality.horizon = None;
        }
        quality
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_effects_off_removes_only_their_budgets() {
        let quality = crate::look::Quality::default();
        assert_eq!(RenderDebug::default().apply(quality), quality);
        let off = RenderDebug {
            ambient_occlusion: false,
            shadows: false,
            ..RenderDebug::default()
        }
        .apply(quality);
        assert!(off.ambient_occlusion.is_none());
        assert!(off.shadows.far.is_none() && off.local_shadows.is_none());
        assert!(off.virtual_shadows.is_none() && off.horizon.is_none());
        // Cascades stay valid (the frame samples none of them instead).
        assert_eq!(off.shadows.cascades, quality.shadows.cascades);
        assert_eq!(off.reflections, quality.reflections);
        off.validate().expect("debug quality stays valid");
    }
}
