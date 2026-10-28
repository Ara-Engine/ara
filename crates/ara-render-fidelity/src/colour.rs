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
    /// AgX, Troy Sobotka's image formation as Blender 5.1 ships it ([`agx`]): 16.5
    /// stops into a log sigmoid that keeps 18% grey and bends bright, saturated light
    /// towards white (Cycles' default view).
    Agx = 3,
    /// AgX with Blender's "Punchy" look: deeper shadows, more contrast.
    AgxPunchy = 4,
    /// AgX with Blender's "Medium High Contrast" look: contrast 1.2 about mid grey.
    AgxMediumHighContrast = 5,
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

/// Blender's AgX looks (`ToneMapper::AgxPunchy`, `AgxMediumHighContrast`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgxLook {
    /// The plain image formation.
    Base,
    /// A shadow curve below mid grey's log value, then CDL power 1.0912.
    Punchy,
    /// Contrast 1.2 about mid grey in AgX Log.
    MediumHighContrast,
}

/// Rows of a 3x3 matrix applied to a column colour.
fn rows(m: [[f32; 3]; 3], c: Vec3) -> Vec3 {
    Vec3::new(
        Vec3::from(m[0]).dot(c),
        Vec3::from(m[1]).dot(c),
        Vec3::from(m[2]).dot(c),
    )
}

/// AgX: linear Rec.709 after exposure to linear display Rec.709, as Blender 5.1's
/// AgX view (EaryChow's formation of Troy Sobotka's AgX) with `look`. An independent
/// analytic form fitted to Blender's own OCIO processor, within 0.5 (mean) and 2
/// (p95) 8-bit display codes of it over 3.5k colours; the shaders (look.slangh
/// `agxCurve`, shaders/post.slang `agx`) carry the same constants.
///
/// Looks apply per channel in Blender's AgX Log space (Rec.2020 through its inset
/// matrix, log2 over 25 stops from 2^-10 * 0.18). The formation insets the colour,
/// encodes log2 over -10..+6.5 stops around 18% grey, shapes it with a two-segment
/// hyperbolic sigmoid (x / (1 + x^p)^(1/p) on each side of mid grey, slope 2.38),
/// outsets, and decodes the Rec.1886 (2.4) code values back to Rec.709.
pub fn agx(colour: Vec3, look: AgxLook) -> Vec3 {
    let mut c = colour;
    if look != AgxLook::Base {
        let v = rows(
            [
                [0.544_814_75, 0.373_787_4, 0.081_397_86],
                [0.140_416_95, 0.754_137_6, 0.105_445_5],
                [0.088_810_42, 0.178_871_76, 0.732_317_8],
            ],
            c,
        )
        .max(Vec3::splat(1e-10));
        let l = (Vec3::new(v.x.log2(), v.y.log2(), v.z.log2()) + Vec3::splat(12.47393)) / 25.;
        let l = match look {
            AgxLook::MediumHighContrast => (l - Vec3::splat(0.4)) * 1.2 + Vec3::splat(0.4),
            _ => l
                .clamp(Vec3::ZERO, Vec3::ONE)
                .to_array()
                .map(|x| {
                    let d = (x - 0.4).min(0.);
                    if x >= 0.4 {
                        x.powf(1.0912)
                    } else {
                        0.367_932_25
                            + d * (0.576_921_4
                                + d * (-13.071_583
                                    + d * (-68.043_43 + d * (-110.187_1 + d * -47.912_39))))
                    }
                })
                .into(),
        };
        let v = (l * 25. - Vec3::splat(12.47393))
            .to_array()
            .map(f32::exp2)
            .into();
        c = rows(
            [
                [2.114_473_3, -1.027_378_5, -0.087_094_925],
                [-0.370_504_27, 1.552_926_2, -0.182_421_92],
                [-0.165_931_4, -0.254_715_53, 1.420_646_9],
            ],
            v,
        );
    }
    let v = rows(
        [
            [0.586_284_65, 0.320_481_4, 0.093_233_96],
            [0.115_577_05, 0.766_820_85, 0.117_602_1],
            [0.090_460_18, 0.189_264_5, 0.720_275_3],
        ],
        c,
    )
    .max(Vec3::splat(1e-10));
    // The sigmoid: pivot (10/16.5, 0.18^(1/2.4)), slope 2.38; toe and shoulder scales
    // make it reach 0 and 1 at the ends.
    let s = v.to_array().map(|x| {
        let l = ((x.log2() + 12.47393) / 16.5).clamp(0., 1.);
        let (scale, power) = if l >= 0.606_060_6 {
            (0.723_107_7, 1.489_164_6)
        } else {
            (0.563_481_7, 1.522_003_8)
        };
        let t = 2.380_159_6 * (l - 0.606_060_6).abs() / scale;
        0.489_437_1
            + (l - 0.606_060_6).signum() * scale * t * (1. + t.powf(power)).powf(-1. / power)
    });
    let o = rows(
        [
            [0.977_084_9, 0.019_273_49, 0.003_641_627],
            [0.017_531_67, 1.012_742_4, -0.030_274_053],
            [0.012_019_836, 0.012_047_557, 0.975_932_6],
        ],
        s.into(),
    )
    .max(Vec3::ZERO)
    .to_array()
    .map(|x| x.powf(2.4));
    rows(
        [
            [1.807_087_2, -0.635_807_6, -0.171_279_6],
            [-0.224_544, 1.237_837_2, -0.013_293_189],
            [-0.184_566_07, -0.281_406_33, 1.465_972_4],
        ],
        o.into(),
    )
    .clamp(Vec3::ZERO, Vec3::ONE)
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
        ToneMapper::Agx => agx(colour, AgxLook::Base),
        ToneMapper::AgxPunchy => agx(colour, AgxLook::Punchy),
        ToneMapper::AgxMediumHighContrast => agx(colour, AgxLook::MediumHighContrast),
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
    /// Blender 5.1's AgX view (display sRGB codes from its OCIO processor) for
    /// greys and colours, per look.
    #[test]
    fn agx_matches_blender() {
        let srgb = |v: f32| {
            if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            }
        };
        let inputs = [
            [0.18, 0.18, 0.18],
            [1., 1., 1.],
            [0.0225, 0.0225, 0.0225],
            [0.6, 0.25, 0.08],
            [0.05, 0.12, 0.4],
            [4., 1.5, 0.3],
        ];
        let blender = [
            (
                ToneMapper::Agx,
                [
                    [0.4613, 0.4613, 0.4614],
                    [0.771, 0.771, 0.771],
                    [0.1357, 0.1357, 0.1358],
                    [0.7187, 0.5177, 0.3429],
                    [0.2363, 0.41, 0.6415],
                    [0.9424, 0.8055, 0.702],
                ],
            ),
            (
                ToneMapper::AgxPunchy,
                [
                    [0.3463, 0.3463, 0.3463],
                    [0.6923, 0.6923, 0.6924],
                    [0.0496, 0.0496, 0.0496],
                    [0.6103, 0.4033, 0.2173],
                    [0.1187, 0.306, 0.5192],
                    [0.9051, 0.7358, 0.6025],
                ],
            ),
            (
                ToneMapper::AgxMediumHighContrast,
                [
                    [0.4613, 0.4613, 0.4614],
                    [0.8138, 0.8138, 0.8138],
                    [0.1004, 0.1004, 0.1005],
                    [0.7625, 0.524, 0.313],
                    [0.1841, 0.3981, 0.6722],
                    [0.9886, 0.8458, 0.7426],
                ],
            ),
        ];
        for (mapper, expected) in blender {
            for (i, (input, want)) in inputs.iter().zip(expected).enumerate() {
                let got = tone_map(Vec3::from(*input), mapper).to_array().map(srgb);
                let error = (Vec3::from(got) - Vec3::from(want)).abs().max_element();
                // In 8-bit codes: greys within 1.5; these saturated colours sit in the
                // fit's tail (its p95 over 3.5k colours is two to four codes).
                let limit = if i < 3 { 1.5 } else { 10. };
                assert!(
                    error * 255. < limit,
                    "{mapper:?} {input:?}: {got:?} vs {want:?}"
                );
            }
        }
        // Monotonic on greys, black to white.
        let mut last = 0.;
        for i in 0..2000 {
            let c = tone_map(Vec3::splat(i as f32 / 50.), ToneMapper::Agx);
            assert!(c.is_finite() && c.x >= last - 1e-6 && c.max_element() <= 1.);
            last = c.x;
        }
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
