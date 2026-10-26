//! Lens effects and screen-space tuning from post-process volumes: depth of field before
//! the temporal resolve (look-dof.slang), motion blur in the post pass's motion variant
//! (look-motion.slangh), chromatic aberration in the post pass, and AO/SSR strength over
//! the quality preset's. Each runs only where the [`super::Quality`] budgets it.

/// The full-frame sensor height focal lengths follow from, in metres.
const SENSOR_HEIGHT: f32 = 0.024;

/// Focal length in metres for a vertical field of view on the full-frame sensor.
fn focal_length(fov: f32) -> f32 {
    0.5 * SENSOR_HEIGHT / (0.5 * fov).tan().max(1e-4)
}

/// A setting a look overrides over the quality preset's own value:
/// `preset × keep + add`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverPreset {
    /// Share of the preset's value that remains.
    pub keep: f32,
    /// Overridden contribution, in the setting's unit.
    pub add: f32,
}
impl Default for OverPreset {
    fn default() -> Self {
        Self { keep: 1., add: 0. }
    }
}
impl OverPreset {
    /// The value over the preset's own.
    pub fn over(self, preset: f32) -> f32 {
        preset * self.keep + self.add
    }
}

/// Thin-lens focus blur.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthOfField {
    /// In-focus distance from the camera, in metres.
    pub focus_distance: f32,
    /// Lens f-number. The focal length follows the camera's vertical field of view on a
    /// 24 mm (full-frame) sensor height.
    pub aperture: f32,
    /// Foreground blur strength, from zero (sharp) to one (physical).
    pub near: f32,
    /// Background blur strength, from zero (sharp) to one (physical).
    pub far: f32,
}
impl DepthOfField {
    /// Shader constants (look.dof): the focus distance, the circle-of-confusion radius
    /// scale in render pixels (radius = scale × (1 − focus / depth)), the largest radius
    /// and the near strength, for a camera with vertical field of view `fov` (radians)
    /// rendering `height` pixels.
    pub(crate) fn constants(self, fov: f32, height: f32, quality: DepthOfFieldQuality) -> [f32; 4] {
        let focus = self.focus_distance.max(focal_length(fov) * 1.01);
        [
            focus,
            self.circle_scale(fov, height),
            quality.max_radius * height,
            self.near,
        ]
    }

    /// Blur radius in pixels at infinity for a camera with vertical field of view
    /// `fov` (radians) rendering `height` pixels: the sensor blur diameter
    /// f² / (N (P − f)) as a radius; a depth d blurs by this times |1 − P / d|.
    pub fn circle_scale(self, fov: f32, height: f32) -> f32 {
        let focal = focal_length(fov);
        let focus = self.focus_distance.max(focal * 1.01);
        focal * focal / (self.aperture * (focus - focal)) / SENSOR_HEIGHT * height * 0.5
    }
}

/// Lens effects of a look.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lens {
    /// Lateral chromatic aberration strength, from zero (off) to one.
    pub chromatic_aberration: f32,
    /// Focus blur; `None` keeps everything sharp.
    pub depth_of_field: Option<DepthOfField>,
    /// Shutter open time as a fraction of a frame (shutter angle / 360°); zero is no
    /// motion blur.
    pub motion_blur: f32,
}

/// Screen-space ambient occlusion tuning over the quality's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AoTuning {
    /// Power on the occlusion: zero none, one physical, up to four exaggerated.
    pub intensity: f32,
    /// Search radius over the preset's, in metres.
    pub radius: OverPreset,
}
impl Default for AoTuning {
    fn default() -> Self {
        Self {
            intensity: 1.,
            radius: OverPreset::default(),
        }
    }
}

/// Screen-space reflection tuning over the quality's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SsrTuning {
    /// Scale on the hit confidence, from zero (probe reflections only) to one.
    pub intensity: f32,
    /// Longest ray over the preset's, in metres.
    pub distance: OverPreset,
}
impl Default for SsrTuning {
    fn default() -> Self {
        Self {
            intensity: 1.,
            distance: OverPreset::default(),
        }
    }
}

/// Depth of field budget (`Quality::depth_of_field`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthOfFieldQuality {
    /// Gather samples per half-resolution pixel, 8 to 96.
    pub samples: u32,
    /// Largest blur radius as a fraction of the render height, 0.005 to 0.05.
    pub max_radius: f32,
}

/// Motion blur budget (`Quality::motion_blur`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionBlurQuality {
    /// Gather samples per moving display pixel, 4 to 32.
    pub samples: u32,
    /// Longest blur in display pixels, 4 to 32 (the 16-pixel velocity tiles and their
    /// neighbours reach that far).
    pub max_length: f32,
}

impl Lens {
    /// Rejects values the shaders cannot represent.
    pub(crate) fn validate(&self, ao: &AoTuning, ssr: &SsrTuning) -> anyhow::Result<()> {
        let preset = |o: OverPreset| o.keep.is_finite() && o.add.is_finite() && o.keep >= 0.;
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.chromatic_aberration)
                && (0.0..=1.0).contains(&self.motion_blur)
                && self.depth_of_field.is_none_or(|d| {
                    d.focus_distance.is_finite()
                        && d.focus_distance > 0.
                        && (0.5..=64.0).contains(&d.aperture)
                        && (0.0..=1.0).contains(&d.near)
                        && (0.0..=1.0).contains(&d.far)
                })
                && (0.0..=4.0).contains(&ao.intensity)
                && preset(ao.radius)
                && (0.0..=1.0).contains(&ssr.intensity)
                && preset(ssr.distance),
            "Invalid lens effects or screen-space tuning"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_circle_of_confusion_follows_the_thin_lens() {
        // 50 mm-equivalent (≈27° vertical) at f/2, focused at 2 m, 1000 pixels tall.
        let focal = 0.05_f32;
        let fov = 2. * (0.5 * SENSOR_HEIGHT / focal).atan();
        let lens = DepthOfField {
            focus_distance: 2.,
            aperture: 2.,
            near: 1.,
            far: 1.,
        };
        let quality = DepthOfFieldQuality {
            samples: 16,
            max_radius: 0.02,
        };
        let [focus, scale, largest, near] = lens.constants(fov, 1000., quality);
        assert_eq!((focus, largest, near), (2., 20., 1.));
        // Diameter at infinity f²/(N(P−f)) = 0.0025/(2·1.95) m on a 24 mm sensor.
        let expected = 0.0025 / (2. * 1.95) / SENSOR_HEIGHT * 1000. * 0.5;
        assert!((scale - expected).abs() < 1e-3, "{scale} vs {expected}");
        assert_eq!(OverPreset { keep: 0.5, add: 2. }.over(4.), 4.);
    }
}
