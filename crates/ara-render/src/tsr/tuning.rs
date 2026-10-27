//! Ara TSR's tuning (`ARA_TSR_*` development knobs, read once when the renderer is
//! created; the list and defaults are in `tsr/mod.rs`). Every value is a record
//! constant or a pass selection, so sweeps need no recook.

/// Which accumulate path resolves a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Path {
    /// The renderer's display history, Catmull-Rom reprojection, display-pixel
    /// Gaussian upsampling, no RCAS.
    #[default]
    Ara,
    /// FSR 3.1's own history pair, Lanczos reprojection and upsampling, then RCAS.
    Fsr,
}

/// The knobs' values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tuning {
    /// `ARA_TSR_PATH` (`ara` or `fsr`); a sharpness above zero selects `fsr`.
    pub path: Path,
    /// `ARA_TSR_SHARPNESS`: FSR's RCAS sharpness, 0 (off, no pass) to 1.
    pub sharpness: f32,
    /// `ARA_TSR_KERNEL_STATIC` / `_MOVING`: the Gaussian's spread (display px⁻²)
    /// for settled and for new or moving pixels; `_GAIN` scales its weight (the
    /// accumulation rate), `ARA_TSR_STILL_TRANSPARENCY` the current frame's weight
    /// on still transparency.
    pub kernel: [f32; 4],
    /// `ARA_TSR_NEW_SURFACE`, `_SAME_SURFACE`, `_REFLECTION`, `_COMPOSITION`
    /// (TsrConstants::signals).
    pub signals: [f32; 4],
    /// `ARA_TSR_AUTOGEN_SCALE`, `_THRESHOLD`, `_VALUE`, `ARA_TSR_PARALLAX`.
    pub autogen: [f32; 4],
    /// `ARA_TSR_ACCUMULATION` (added per frame), `_MIN_DISOCCLUSION`.
    pub accumulation: [f32; 2],
    /// `ARA_TSR_VELOCITY`, `_REACTIVENESS`, `_SHADING_CHANGE` (FSR's factor and
    /// scales; a shading change scale of 0 skips its pyramid pass).
    pub factors: [f32; 3],
    /// `ARA_TSR_LUMA_INSTABILITY` (0 skips the pass).
    pub luma_instability: bool,
    /// `ARA_TSR_PREEXPOSED`: the HDR input already holds radiance × the applied
    /// exposure (the pre-exposure contract); otherwise TSR applies it.
    pub pre_exposed: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            path: Path::Ara,
            sharpness: 0.,
            // TAAU's proven area-like spreads; gain 1 keeps FSR's accumulation rate.
            kernel: [5.56, 2.78, 1., 0.5],
            signals: [1., 4., 0.25, 0.5],
            // FSR's autogen as AMD's sample dispatches it; full reactivity at one
            // render pixel of parallax.
            autogen: [1., 0.2, 0.9, 1.],
            // FSR 3.1's context defaults.
            accumulation: [1. / 3., -1. / 3.],
            factors: [1., 1., 1.],
            luma_instability: true,
            pre_exposed: false,
        }
    }
}

impl Tuning {
    /// The defaults overridden by any `ARA_TSR_*` variable set.
    pub(crate) fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// `from_env` over any variable source (tests).
    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let number = |name: &str, default: f32| {
            get(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        let d = Self::default();
        let sharpness = number("ARA_TSR_SHARPNESS", d.sharpness).clamp(0., 1.);
        let fsr = get("ARA_TSR_PATH").is_some_and(|v| v.trim().eq_ignore_ascii_case("fsr"));
        Self {
            path: if fsr || sharpness > 0. {
                Path::Fsr
            } else {
                Path::Ara
            },
            sharpness,
            kernel: [
                number("ARA_TSR_KERNEL_STATIC", d.kernel[0]).max(0.01),
                number("ARA_TSR_KERNEL_MOVING", d.kernel[1]).max(0.01),
                number("ARA_TSR_KERNEL_GAIN", d.kernel[2]).max(0.),
                number("ARA_TSR_STILL_TRANSPARENCY", d.kernel[3]).clamp(0., 1.),
            ],
            signals: [
                number("ARA_TSR_NEW_SURFACE", d.signals[0]).clamp(0., 1.),
                number("ARA_TSR_SAME_SURFACE", d.signals[1]).max(0.),
                number("ARA_TSR_REFLECTION", d.signals[2]).clamp(0., 1.),
                number("ARA_TSR_COMPOSITION", d.signals[3]).clamp(0., 1.),
            ],
            autogen: [
                number("ARA_TSR_AUTOGEN_SCALE", d.autogen[0]).max(0.),
                number("ARA_TSR_AUTOGEN_THRESHOLD", d.autogen[1]).max(0.),
                number("ARA_TSR_AUTOGEN_VALUE", d.autogen[2]).clamp(0., 1.),
                number("ARA_TSR_PARALLAX", d.autogen[3]).max(0.01),
            ],
            accumulation: [
                number("ARA_TSR_ACCUMULATION", d.accumulation[0]).clamp(0., 1.),
                number("ARA_TSR_MIN_DISOCCLUSION", d.accumulation[1]).clamp(-1., 1.),
            ],
            factors: [
                number("ARA_TSR_VELOCITY", d.factors[0]).clamp(0., 1.),
                number("ARA_TSR_REACTIVENESS", d.factors[1]).max(0.),
                number("ARA_TSR_SHADING_CHANGE", d.factors[2]).max(0.),
            ],
            luma_instability: number("ARA_TSR_LUMA_INSTABILITY", 1.) > 0.,
            pre_exposed: number("ARA_TSR_PREEXPOSED", 0.) > 0.,
        }
    }

    /// The accumulate path this tuning resolves with (RCAS needs FSR's history).
    pub(crate) fn path(&self) -> Path {
        if self.sharpness > 0. {
            Path::Fsr
        } else {
            self.path
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_override_defaults_and_sharpness_selects_fsr() {
        let tuning = Tuning::from_lookup(|name| match name {
            "ARA_TSR_KERNEL_STATIC" => Some("7".into()),
            "ARA_TSR_SHARPNESS" => Some("0.5".into()),
            "ARA_TSR_LUMA_INSTABILITY" => Some("0".into()),
            _ => None,
        });
        assert_eq!(tuning.kernel[0], 7.);
        assert_eq!(tuning.path(), Path::Fsr);
        assert!(!tuning.luma_instability);
        let default = Tuning::from_lookup(|_| None);
        assert_eq!(default, Tuning::default());
        assert_eq!(default.path(), Path::Ara);
    }
}
