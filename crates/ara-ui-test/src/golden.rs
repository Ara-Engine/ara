use crate::Result;
use tiny_skia::Pixmap;

/// Independent limits prevent a small severe regression from hiding inside a global average.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// Maximum mean local 8x8 luminance SSIM loss (zero is identical).
    pub max_ssim_loss: f64,
    /// Maximum mean weighted RGB/alpha error in 0..=1.
    pub max_mean_error: f64,
    /// Maximum fraction of pixels exceeding the per-pixel threshold.
    pub max_changed_fraction: f64,
    /// Maximum weighted RGB/alpha error before a pixel counts as changed.
    pub per_pixel_threshold: f64,
}
impl Default for Tolerance {
    fn default() -> Self {
        Self {
            max_ssim_loss: 0.01,
            max_mean_error: 0.003,
            max_changed_fraction: 0.005,
            per_pixel_threshold: 0.02,
        }
    }
}
/// Perceptual snapshot metrics plus an amplified magenta difference image.
pub struct Comparison {
    /// True only when dimensions and all three error budgets pass.
    pub pass: bool,
    /// Mean local SSIM loss.
    pub ssim_loss: f64,
    /// Mean weighted color/alpha error.
    pub mean_error: f64,
    /// Fraction of pixels over the configured perceptual threshold.
    pub changed_fraction: f64,
    /// Encoded RGBA PNG, highlighting changed and dimension-mismatch regions.
    pub diff_png: Vec<u8>,
}

/// Compare decoded premultiplied PNGs using local SSIM and color/alpha error budgets.
/// Different dimensions always fail; transparent-color changes are compared as rendered pixels.
pub fn compare(expected: &[u8], actual: &[u8], tolerance: Tolerance) -> Result<Comparison> {
    if [
        tolerance.max_ssim_loss,
        tolerance.max_mean_error,
        tolerance.max_changed_fraction,
        tolerance.per_pixel_threshold,
    ]
    .iter()
    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("golden tolerances must be finite and in 0..=1".into());
    }
    let a = Pixmap::decode_png(expected)?;
    let b = Pixmap::decode_png(actual)?;
    let width = a.width().max(b.width());
    let height = a.height().max(b.height());
    if u64::from(width) * u64::from(height) > 16_777_216 {
        return Err("golden comparison exceeds 16 megapixels".into());
    }
    let mut diff = Pixmap::new(width, height).ok_or("invalid diff dimensions")?;
    let mut changed = 0;
    let mut error_sum = 0.0;
    for y in 0..height {
        for x in 0..width {
            let pa = pixel(&a, x, y);
            let pb = pixel(&b, x, y);
            let delta = [0, 1, 2, 3].map(|i| (pa[i] - pb[i]).abs());
            let error = (0.2126 * delta[0] + 0.7152 * delta[1] + 0.0722 * delta[2]).max(delta[3]);
            let missing = x >= a.width().min(b.width()) || y >= a.height().min(b.height());
            error_sum += if missing { 1.0 } else { error };
            if missing || error > tolerance.per_pixel_threshold {
                changed += 1;
            }
            let v = if missing {
                255
            } else {
                (error * 1020.0).round().min(255.0) as u8
            };
            let offset = ((y * width + x) * 4) as usize;
            diff.data_mut()[offset..offset + 4].copy_from_slice(&[v, 0, v, 255]);
        }
    }
    let mut ssim_loss = 0.0;
    let mut blocks = 0;
    for y in (0..height).step_by(8) {
        for x in (0..width).step_by(8) {
            let mut values = Vec::with_capacity(64);
            for by in y..(y + 8).min(height) {
                for bx in x..(x + 8).min(width) {
                    let luma = |p: [f64; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
                    values.push((luma(pixel(&a, bx, by)), luma(pixel(&b, bx, by))));
                }
            }
            let n = values.len() as f64;
            let ma = values.iter().map(|v| v.0).sum::<f64>() / n;
            let mb = values.iter().map(|v| v.1).sum::<f64>() / n;
            let va = values.iter().map(|v| (v.0 - ma).powi(2)).sum::<f64>() / n;
            let vb = values.iter().map(|v| (v.1 - mb).powi(2)).sum::<f64>() / n;
            let cov = values.iter().map(|v| (v.0 - ma) * (v.1 - mb)).sum::<f64>() / n;
            let ssim = ((2.0 * ma * mb + 0.0001) * (2.0 * cov + 0.0009))
                / ((ma * ma + mb * mb + 0.0001) * (va + vb + 0.0009));
            ssim_loss += (1.0 - ssim).clamp(0.0, 1.0);
            blocks += 1;
        }
    }
    let pixels = f64::from(width) * f64::from(height);
    let mean_error = error_sum / pixels;
    let changed_fraction = f64::from(changed) / pixels;
    let ssim_loss = ssim_loss / f64::from(blocks);
    Ok(Comparison {
        pass: a.width() == b.width()
            && a.height() == b.height()
            && ssim_loss <= tolerance.max_ssim_loss
            && mean_error <= tolerance.max_mean_error
            && changed_fraction <= tolerance.max_changed_fraction,
        ssim_loss,
        mean_error,
        changed_fraction,
        diff_png: diff.encode_png()?,
    })
}
fn pixel(p: &Pixmap, x: u32, y: u32) -> [f64; 4] {
    if x >= p.width() || y >= p.height() {
        return [0.0; 4];
    }
    let i = ((y * p.width() + x) * 4) as usize;
    [
        p.data()[i],
        p.data()[i + 1],
        p.data()[i + 2],
        p.data()[i + 3],
    ]
    .map(|v| f64::from(v) / 255.0)
}
