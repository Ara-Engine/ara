//! Display mapping and histogram-metering contracts, independent of transport.
use glam::Vec3;

/// Explicit offline readback of the photographic exposure controller.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ExposureState {
    /// Multiplier applied to physical scene radiance before tone mapping.
    pub multiplier: f32,
    /// Preceding frame multiplier, used by display-history rejection.
    pub previous_multiplier: f32,
    /// Clamped target EV100, before adaptation and automatic compensation.
    pub target_ev100: f32,
    /// Percentile-trimmed geometric mean scene luminance; zero in manual mode.
    pub metered_luminance: f32,
}

/// SDR display transform applied after photographic exposure, before grading/sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ToneMapper {
    /// Narkowicz's ACES fitted curve, retained for existing captures.
    AcesFit = 0,
    /// Khronos PBR Neutral; preserves midtone colour and compresses highlights.
    PbrNeutral = 1,
    /// Hill's fitted ACES RRT+ODT in AP1, pre-scaled so 18% grey stays 18%: a soft
    /// toe and desaturating shoulder, close to Unreal's default filmic curve.
    Filmic = 2,
}

/// Scene-linear colour grading applied after exposure, before the tone mapper.
/// The default is the identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grading {
    /// Colour temperature in kelvin that renders as neutral white (6500 is identity).
    pub white_balance: f32,
    /// Saturation; 0 is greyscale, 1 unchanged, up to 2.
    pub saturation: f32,
    /// Power contrast about 18% grey; 1 unchanged, from 0.5 to 2.
    pub contrast: f32,
    /// Corner darkening strength, from zero (off) to one.
    pub vignette: f32,
}
impl Default for Grading {
    fn default() -> Self {
        Self {
            white_balance: 6500.,
            saturation: 1.,
            contrast: 1.,
            vignette: 0.,
        }
    }
}
impl Grading {
    /// Whether every control is within its documented range.
    pub fn is_valid(&self) -> bool {
        (1000.0..=40000.0).contains(&self.white_balance)
            && (0.0..=2.0).contains(&self.saturation)
            && (0.5..=2.0).contains(&self.contrast)
            && (0.0..=1.0).contains(&self.vignette)
    }
    /// Per-channel linear Rec.709 gains that map a Planckian white of
    /// `white_balance` kelvin to neutral, at unchanged luminance.
    pub fn white_balance_gain(&self) -> Vec3 {
        if self.white_balance == 6500. {
            return Vec3::ONE;
        }
        let reference = planckian_rgb(6500.);
        let gain = reference / planckian_rgb(self.white_balance);
        gain / luminance(gain)
    }
    /// Applies white balance, contrast and saturation to exposed scene-linear colour
    /// (the vignette depends on screen position and is applied by the post pass).
    pub fn apply(&self, colour: Vec3) -> Vec3 {
        let c = colour.max(Vec3::ZERO) * self.white_balance_gain();
        let c = if self.contrast == 1. {
            c
        } else {
            (c / 0.18).powf(self.contrast) * 0.18
        };
        Vec3::splat(luminance(c))
            .lerp(c, self.saturation)
            .max(Vec3::ZERO)
    }
}

/// Rec.709 relative luminance.
pub fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// Linear Rec.709 colour of a Planckian radiator at unit luminance, using Kim et
/// al.'s cubic fit of the CIE 1931 locus (valid 1667–25000 K; clamped outside).
fn planckian_rgb(kelvin: f32) -> Vec3 {
    let t = kelvin.clamp(1667., 25000.) as f64;
    let x = if t <= 4000. {
        -0.2661239e9 / (t * t * t) - 0.2343589e6 / (t * t) + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / (t * t * t) + 2.1070379e6 / (t * t) + 0.2226347e3 / t + 0.240390
    };
    let y = if t <= 2222. {
        -1.1063814 * x * x * x - 1.34811020 * x * x + 2.18555832 * x - 0.20219683
    } else if t <= 4000. {
        -0.9549476 * x * x * x - 1.37418593 * x * x + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x * x * x - 5.87338670 * x * x + 3.75112997 * x - 0.37001483
    };
    let (cx, cz) = (x / y, (1. - x - y) / y);
    let rgb = Vec3::new(
        (3.2406 * cx - 1.5372 - 0.4986 * cz) as f32,
        (-0.9689 * cx + 1.8758 + 0.0415 * cz) as f32,
        (0.0557 * cx - 0.2040 + 1.0570 * cz) as f32,
    );
    rgb.max(Vec3::splat(1e-4))
}

/// Exposure pre-scale that keeps 18% grey at 18% through the fitted ACES curve.
pub const FILMIC_SCALE: f32 = 1.464;

fn filmic(colour: Vec3) -> Vec3 {
    let c = colour * FILMIC_SCALE;
    let input = glam::Mat3::from_cols(
        Vec3::new(0.59719, 0.07600, 0.02840),
        Vec3::new(0.35458, 0.90834, 0.13383),
        Vec3::new(0.04823, 0.01566, 0.83777),
    );
    let output = glam::Mat3::from_cols(
        Vec3::new(1.60475, -0.10208, -0.00327),
        Vec3::new(-0.53108, 1.10813, -0.07276),
        Vec3::new(-0.07367, -0.00605, 1.07602),
    );
    let v = input * c;
    let fitted = (v * (v + Vec3::splat(0.0245786)) - Vec3::splat(0.000090537))
        / (v * (v * 0.983729 + Vec3::splat(0.432_951)) + Vec3::splat(0.238081));
    (output * fitted).clamp(Vec3::ZERO, Vec3::ONE)
}

/// Percentile-trimmed logarithmic luminance metering, with photographic EV limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoExposure {
    /// Ignore this lower fraction of the histogram, from zero through one.
    pub low_percentile: f32,
    /// Ignore samples above this histogram fraction; greater than low_percentile.
    pub high_percentile: f32,
    /// Lowest permitted camera EV100 before exposure compensation.
    pub minimum_ev100: f32,
    /// Highest permitted camera EV100 before exposure compensation.
    pub maximum_ev100: f32,
}
impl Default for AutoExposure {
    fn default() -> Self {
        Self {
            low_percentile: 0.05,
            high_percentile: 0.95,
            minimum_ev100: -6.,
            maximum_ev100: 20.,
        }
    }
}

/// Map nonnegative linear Rec.709 radiance after exposure to linear SDR Rec.709.
/// Implements the Khronos PBR Neutral specification, the fitted ACES filmic curve,
/// or the legacy per-channel ACES fit.
pub fn tone_map(colour: Vec3, mapper: ToneMapper) -> Vec3 {
    let colour = colour.max(Vec3::ZERO);
    match mapper {
        ToneMapper::AcesFit => ((colour * (colour * 2.51 + Vec3::splat(0.03)))
            / (colour * (colour * 2.43 + Vec3::splat(0.59)) + Vec3::splat(0.14)))
        .clamp(Vec3::ZERO, Vec3::ONE),
        ToneMapper::Filmic => filmic(colour),
        ToneMapper::PbrNeutral => {
            let floor = colour.min_element();
            let offset = if floor < 0.08 {
                floor - 6.25 * floor * floor
            } else {
                0.04
            };
            let colour = colour - Vec3::splat(offset);
            let peak = colour.max_element();
            if peak <= 0.76 {
                return colour;
            }
            let compressed = 1. - 0.24 * 0.24 / (peak - 0.52);
            let neutral = 1. - 1. / (1. + 0.15 * (peak - compressed));
            (colour * (compressed / peak)).lerp(Vec3::splat(compressed), neutral)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_preserves_midtone_chroma_and_rolls_to_white() {
        assert_eq!(tone_map(Vec3::ZERO, ToneMapper::PbrNeutral), Vec3::ZERO);
        let c = tone_map(Vec3::new(0.3, 0.5, 0.7), ToneMapper::PbrNeutral);
        assert!((c - Vec3::new(0.26, 0.46, 0.66)).length() < 1e-6);
        let mut last = 0.;
        for i in 0..1000 {
            let c = tone_map(Vec3::splat(i as f32 / 10.), ToneMapper::PbrNeutral);
            assert!(c.is_finite() && c.min_element() >= last && c.max_element() <= 1.);
            last = c.x;
        }
    }
    #[test]
    fn filmic_keeps_mid_grey_and_is_monotonic() {
        let grey = tone_map(Vec3::splat(0.18), ToneMapper::Filmic);
        assert!(
            (grey - Vec3::splat(0.18)).abs().max_element() < 0.004,
            "{grey}"
        );
        let mut last = 0.;
        for i in 1..2000 {
            let c = tone_map(Vec3::splat(i as f32 / 50.), ToneMapper::Filmic);
            assert!(c.is_finite() && c.x >= last && c.max_element() <= 1.);
            last = c.x;
        }
        assert!(tone_map(Vec3::splat(40.), ToneMapper::Filmic).x > 0.97);
    }
    #[test]
    fn grading_identity_and_white_balance() {
        let g = Grading::default();
        assert!((g.white_balance_gain() - Vec3::ONE).abs().max_element() < 1e-5);
        let c = Vec3::new(0.3, 0.2, 0.05);
        assert!((g.apply(c) - c).abs().max_element() < 1e-6);
        // Balancing for tungsten makes a tungsten-coloured white neutral.
        let tungsten = Grading {
            white_balance: 3200.,
            ..g
        };
        let lamp = planckian_rgb(3200.) / planckian_rgb(6500.);
        let neutral = lamp * tungsten.white_balance_gain();
        assert!(
            (neutral.x - neutral.z).abs() / neutral.y < 1e-4,
            "{neutral}"
        );
        let grey = Grading {
            saturation: 0.,
            ..g
        }
        .apply(c);
        assert!((grey.x - grey.z).abs() < 1e-6);
    }
}
