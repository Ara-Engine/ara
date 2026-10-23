//! Explicit display encoding, independent of window and rendering backends.
#![deny(missing_docs)]
use serde::{Deserialize, Serialize};

/// Requested desktop output; unsupported HDR must produce a diagnostic.
#[repr(u32)]
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ara_schema::Schema,
)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum OutputMode {
    /// sRGB transfer and BT.709 primaries, the default.
    #[default]
    Sdr,
    /// Absolute ST2084 PQ with BT.2020 primaries in a 10-bit swapchain.
    Hdr10,
    /// Linear extended BT.709 in a floating-point swapchain, 1 = 80 nits.
    ScRgb,
}
/// Display settings, separate from camera exposure and scene lighting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ara_schema::Schema)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OutputSettings {
    /// Requested swapchain transfer and primaries. SDR remains the default.
    #[ara(default = "sdr")]
    pub mode: OutputMode,
    /// Luminance of diffuse white in nits, after the creative tone mapper.
    #[ara(default = 203, range(min = 80, max = 500))]
    pub paper_white: f32,
    /// Display peak luminance; the HDR shoulder rolls highlights toward this limit.
    #[ara(default = 1000, range(min = 80, max = 10000))]
    pub peak_nits: f32,
}
impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            mode: OutputMode::Sdr,
            paper_white: 203.,
            peak_nits: 1000.,
        }
    }
}
impl OutputSettings {
    /// Rejects nonfinite settings and a peak below diffuse white.
    pub fn valid(self) -> bool {
        self.paper_white.is_finite()
            && self.peak_nits.is_finite()
            && (80.0..=500.0).contains(&self.paper_white)
            && (self.paper_white..=10000.0).contains(&self.peak_nits)
    }
    /// Applies a hue-preserving highlight shoulder to exposed linear BT.709.
    /// The creative PBR Neutral/AgX operator may be supplied upstream. Values
    /// through paper white remain linear in HDR; only highlights are compressed.
    pub fn tone_map(self, rgb: [f32; 3]) -> [f32; 3] {
        let maximum = rgb.into_iter().fold(0f32, f32::max);
        let ceiling = if self.mode == OutputMode::Sdr {
            1.0
        } else {
            self.peak_nits / self.paper_white
        };
        let start = if self.mode == OutputMode::Sdr {
            0.8
        } else {
            1.0
        };
        if maximum <= start {
            return rgb;
        }
        let headroom = (ceiling - start).max(1e-6);
        let mapped = start + headroom * (1.0 - (-(maximum - start) / headroom).exp());
        rgb.map(|v| v * mapped / maximum)
    }
    /// Encodes display-linear BT.709 relative to paper white, after tone mapping.
    pub fn encode(self, rgb: [f32; 3]) -> [f32; 3] {
        match self.mode {
            OutputMode::Sdr => rgb.map(srgb),
            OutputMode::ScRgb => rgb.map(|v| v * self.paper_white / 80.0),
            OutputMode::Hdr10 => bt2020(rgb).map(|v| pq(v * self.paper_white)),
        }
    }
}
/// Linear BT.709 to linear BT.2020, D65 white point.
pub fn bt2020(v: [f32; 3]) -> [f32; 3] {
    [
        0.627404 * v[0] + 0.329283 * v[1] + 0.043313 * v[2],
        0.069097 * v[0] + 0.919540 * v[1] + 0.011362 * v[2],
        0.016391 * v[0] + 0.088013 * v[1] + 0.895595 * v[2],
    ]
}
/// ST2084 forward transfer of absolute luminance in nits, bounded to 10,000.
pub fn pq(nits: f32) -> f32 {
    let y = (nits / 10000.0).clamp(0.0, 1.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * y) / (1.0 + 2392.0 / 128.0 * y)).powf(2523.0 / 32.0)
}
/// Standard sRGB forward transfer, including its linear toe.
pub fn srgb(v: f32) -> f32 {
    let v = v.clamp(0., 1.);
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_hdr_reference_values_and_white_point() {
        assert!((pq(100.) - 0.5080784).abs() < 0.00001);
        assert!((pq(1000.) - 0.7518271).abs() < 0.00001);
        assert!((pq(10000.) - 1.).abs() < 0.00001);
        let settings = OutputSettings {
            mode: OutputMode::ScRgb,
            ..Default::default()
        };
        assert_eq!(settings.encode([1.; 3]), [2.5375; 3]);
        for v in bt2020([1.; 3]) {
            assert!((v - 1.).abs() < 0.00001);
        }
        assert_eq!(OutputSettings::default().mode, OutputMode::Sdr);
        assert!(
            !OutputSettings {
                paper_white: f32::NAN,
                ..settings
            }
            .valid()
        );
    }
    #[test]
    fn hdr_shoulder_preserves_diffuse_white_and_bounds_highlights() {
        let s = OutputSettings {
            mode: OutputMode::Hdr10,
            ..Default::default()
        };
        assert_eq!(s.tone_map([1., 0.5, 0.2]), [1., 0.5, 0.2]);
        let mapped = s.tone_map([1000., 500., 200.]);
        assert!(mapped[0] <= s.peak_nits / s.paper_white);
        assert_eq!(mapped[1] / mapped[0], 0.5);
    }
}
