//! E3's quality lines (R4-2 / R5-1 `score-finite.py`), against the exact finite-range
//! control, per frame and accumulated over a run:
//! - leak: pixels whose radiance exceeds the control's by more than 3% of the
//!   control's luminance (floored at 2% of its mean);
//! - pops: 16x16 tiles whose mean |change(test) - change(control)| between frames
//!   exceeds 2% of the control's tile mean (same floor);
//! - highlights: on metal, pixels brighter than twice the metal median keep
//!   sum(test) / sum(control) of their energy;
//! - shadows: where blockers darken the control by more than 10% of its no-blocker
//!   luminance (on non-blocker surfaces), the test's darkening over the control's.
//!
//! Plus the largest per-pixel relative excess and the radiance L1 error.
use serde::Serialize;

/// Luminance of an RGBA pixel.
fn lum(p: &[f32]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

/// One frame's images (RGBA32F radiance, four floats per pixel) and surface classes.
pub(crate) struct Images<'a> {
    pub test: &'a [f32],
    pub control: &'a [f32],
    /// No-blocker test and control (shadow line), when scored.
    pub test_open: Option<&'a [f32]>,
    pub control_open: Option<&'a [f32]>,
    /// Per pixel: covered, metal, blocker surface.
    pub covered: &'a [bool],
    pub metal: &'a [bool],
    pub blocker: &'a [bool],
}

/// Accumulated E3 lines.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct E3 {
    pub frames: u32,
    pub leak_pixels: u64,
    pub covered_pixel_frames: u64,
    pub pops: u64,
    pub tile_frames: u64,
    pub max_excess_change: f64,
    pub highlight_test: f64,
    pub highlight_control: f64,
    pub highlight_pixels: u64,
    pub shadow_test: f64,
    pub shadow_control: f64,
    pub shadow_pixels: u64,
    /// Largest (test - control) / max(control, floor) over every pixel and frame.
    pub max_relative_excess: f64,
    /// Largest |test - control| / max(control, floor) (deficit included).
    pub max_relative_error: f64,
    /// Largest per-frame sum |test - control| / sum control.
    pub radiance_l1_max: f64,
    #[serde(skip)]
    previous: Option<(Vec<f64>, Vec<f64>)>,
}

impl E3 {
    pub(crate) fn highlight_retention(&self) -> f64 {
        if self.highlight_control > 0. {
            self.highlight_test / self.highlight_control
        } else {
            1.
        }
    }
    pub(crate) fn shadow_retention(&self) -> f64 {
        if self.shadow_control > 0. {
            self.shadow_test / self.shadow_control
        } else {
            1.
        }
    }

    /// Scores one frame of `width` x `height`.
    pub(crate) fn add(&mut self, width: usize, height: usize, im: &Images) {
        let n = width * height;
        let lt: Vec<f64> = (0..n).map(|i| lum(&im.test[i * 4..])).collect();
        let lc: Vec<f64> = (0..n).map(|i| lum(&im.control[i * 4..])).collect();
        let covered = im.covered.iter().filter(|&&c| c).count();
        let mean = (0..n)
            .filter(|&i| im.covered[i])
            .map(|i| lc[i])
            .sum::<f64>()
            / covered.max(1) as f64;
        let floor = 0.02 * mean.max(1e-30);
        let (mut diff, mut total) = (0., 0.);
        for i in (0..n).filter(|&i| im.covered[i]) {
            let excess = (lt[i] - lc[i]) / lc[i].max(floor);
            if excess > 0.03 {
                self.leak_pixels += 1;
            }
            self.max_relative_excess = self.max_relative_excess.max(excess);
            self.max_relative_error = self.max_relative_error.max(excess.abs());
            for k in 0..3 {
                diff += f64::from((im.test[i * 4 + k] - im.control[i * 4 + k]).abs());
                total += f64::from(im.control[i * 4 + k]);
            }
        }
        self.covered_pixel_frames += covered as u64;
        self.radiance_l1_max = self
            .radiance_l1_max
            .max(if total > 0. { diff / total } else { 0. });
        // Highlights: metal pixels above twice the metal median.
        let mut metal: Vec<f64> = (0..n)
            .filter(|&i| im.covered[i] && im.metal[i])
            .map(|i| lc[i])
            .collect();
        if !metal.is_empty() {
            metal.sort_by(f64::total_cmp);
            let median = metal[metal.len() / 2];
            for i in (0..n).filter(|&i| im.covered[i] && im.metal[i] && lc[i] > 2. * median) {
                self.highlight_test += lt[i];
                self.highlight_control += lc[i];
                self.highlight_pixels += 1;
            }
        }
        // Shadows: darkening the blockers cause on other surfaces.
        if let (Some(to), Some(co)) = (im.test_open, im.control_open) {
            for i in (0..n).filter(|&i| im.covered[i] && !im.blocker[i]) {
                let (open_c, open_t) = (lum(&co[i * 4..]), lum(&to[i * 4..]));
                let dark_c = open_c - lc[i];
                if dark_c > 0.1 * open_c {
                    self.shadow_test += open_t - lt[i];
                    self.shadow_control += dark_c;
                    self.shadow_pixels += 1;
                }
            }
        }
        // Pops: excess temporal change per 16x16 tile.
        if let Some((pt, pc)) = &self.previous {
            for ty in (0..height).step_by(16) {
                for tx in (0..width).step_by(16) {
                    let (mut delta, mut base, mut count) = (0., 0., 0usize);
                    for y in ty..(ty + 16).min(height) {
                        for x in tx..(tx + 16).min(width) {
                            let i = y * width + x;
                            delta += ((lt[i] - pt[i]) - (lc[i] - pc[i])).abs();
                            base += lc[i];
                            count += 1;
                        }
                    }
                    let ratio = (delta / count as f64) / (base / count as f64).max(floor);
                    if ratio > 0.02 {
                        self.pops += 1;
                    }
                    self.max_excess_change = self.max_excess_change.max(ratio);
                    self.tile_frames += 1;
                }
            }
        }
        self.previous = Some((lt, lc));
        self.frames += 1;
    }

    /// Whether every line is exact: no leak, no pop, full highlight and shadow energy
    /// (to 1e-4).
    pub(crate) fn exact(&self) -> bool {
        self.leak_pixels == 0
            && self.pops == 0
            && (self.highlight_retention() - 1.).abs() <= 1e-4
            && (self.shadow_retention() - 1.).abs() <= 1e-4
    }
}

/// Writes `rgba` (radiance, `size`) as an sRGB PNG at most 1920 wide: exposure for
/// `ev100` (1 / (1.2 * 2^EV100)), the Narkowicz ACES fit, bilinear downscale.
pub(crate) fn png(
    path: &std::path::Path,
    rgba: &[f32],
    size: [u32; 2],
    ev100: f32,
) -> anyhow::Result<()> {
    let exposure = 1. / (1.2 * 2f32.powf(ev100));
    map_png(path, size, |i| {
        let tone = |x: f32| {
            let x = (x * exposure).max(0.);
            ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0., 1.)
        };
        [
            tone(rgba[i * 4]),
            tone(rgba[i * 4 + 1]),
            tone(rgba[i * 4 + 2]),
        ]
    })
}

/// Writes a false-colour PNG of a scalar per pixel (`value`, 0..`max`), black to
/// blue, green, yellow, red.
pub(crate) fn heat_png(
    path: &std::path::Path,
    size: [u32; 2],
    max: f32,
    value: impl Fn(usize) -> f32,
) -> anyhow::Result<()> {
    map_png(path, size, |i| {
        let t = (value(i) / max.max(1e-30)).clamp(0., 1.) * 4.;
        let stops = [
            [0., 0., 0.],
            [0., 0., 1.],
            [0., 1., 0.],
            [1., 1., 0.],
            [1., 0., 0.],
        ];
        let k = (t.floor() as usize).min(3);
        let f = t - k as f32;
        let (a, b) = (stops[k], stops[k + 1]);
        [
            a[0] + (b[0] - a[0]) * f,
            a[1] + (b[1] - a[1]) * f,
            a[2] + (b[2] - a[2]) * f,
        ]
    })
}

/// Downscales (bilinear) to at most 1920 wide and writes linear-display colours as
/// an 8-bit PNG (`colour` already display-encoded, 0..1).
fn map_png(
    path: &std::path::Path,
    size: [u32; 2],
    colour: impl Fn(usize) -> [f32; 3],
) -> anyhow::Result<()> {
    let [w, h] = size;
    let scale = (1920. / w as f32).min(1080. / h as f32).min(1.);
    let (ow, oh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let full: Vec<[f32; 3]> = (0..(w * h) as usize).map(&colour).collect();
    let mut out = image::RgbImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let sx = ((x as f32 + 0.5) / scale - 0.5).clamp(0., (w - 1) as f32);
            let sy = ((y as f32 + 0.5) / scale - 0.5).clamp(0., (h - 1) as f32);
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let at = |x: u32, y: u32| full[(y * w + x) as usize];
            let mut c = [0f32; 3];
            for (k, v) in c.iter_mut().enumerate() {
                let top = at(x0, y0)[k] * (1. - fx) + at(x1, y0)[k] * fx;
                let bottom = at(x0, y1)[k] * (1. - fx) + at(x1, y1)[k] * fx;
                *v = top * (1. - fy) + bottom * fy;
            }
            let srgb = |v: f32| {
                let v = v.clamp(0., 1.);
                let s = if v <= 0.0031308 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1. / 2.4) - 0.055
                };
                (s * 255. + 0.5) as u8
            };
            out.put_pixel(x, y, image::Rgb(c.map(srgb)));
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    out.save(path)?;
    Ok(())
}
