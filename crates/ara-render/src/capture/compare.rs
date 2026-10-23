use anyhow::{Result, ensure};
use image::{Rgba, RgbaImage};

/// Perceptual comparison result. Score is mean local RGB (1 - SSIM), lower is better.
pub struct Comparison {
    /// Dissimilarity score; identical images score zero.
    pub score: f64,
    /// Whether score is within the requested inclusive tolerance.
    pub passed: bool,
    /// Absolute RGB error, amplified four times for visibility.
    pub diff: RgbaImage,
}
/// Compare using non-overlapping 8x8 local SSIM windows on normalized display RGB.
/// Uses Wang et al.'s C1=(0.01L)^2 and C2=(0.03L)^2 with L=1.
/// Edge windows use their actual sample count; alpha does not affect the metric.
pub fn compare(actual: &RgbaImage, golden: &RgbaImage, tolerance: f64) -> Result<Comparison> {
    ensure!(
        actual.dimensions() == golden.dimensions(),
        "Golden dimensions differ"
    );
    ensure!(actual.width() > 0 && actual.height() > 0, "Empty golden");
    ensure!(
        tolerance.is_finite() && (0.0..=2.0).contains(&tolerance),
        "Invalid SSIM tolerance"
    );
    // Avoid cancellation in zero-variance windows and preserve the exact identity contract.
    if actual
        .pixels()
        .zip(golden.pixels())
        .all(|(a, b)| a.0[..3] == b.0[..3])
    {
        return Ok(Comparison {
            score: 0.,
            passed: true,
            diff: RgbaImage::from_pixel(actual.width(), actual.height(), Rgba([0, 0, 0, 255])),
        });
    }
    let mut sum = 0.;
    let mut weight = 0.;
    for y in (0..actual.height()).step_by(8) {
        for x in (0..actual.width()).step_by(8) {
            let xmax = (x + 8).min(actual.width());
            let ymax = (y + 8).min(actual.height());
            let n = f64::from((xmax - x) * (ymax - y));
            for channel in 0..3 {
                let mut a = 0.;
                let mut b = 0.;
                let mut aa = 0.;
                let mut bb = 0.;
                let mut ab = 0.;
                for py in y..ymax {
                    for px in x..xmax {
                        let av = f64::from(actual.get_pixel(px, py)[channel]) / 255.;
                        let bv = f64::from(golden.get_pixel(px, py)[channel]) / 255.;
                        a += av;
                        b += bv;
                        aa += av * av;
                        bb += bv * bv;
                        ab += av * bv;
                    }
                }
                let ma = a / n;
                let mb = b / n;
                let denominator = (n - 1.).max(1.);
                let va = ((aa - a * a / n) / denominator).max(0.);
                let vb = ((bb - b * b / n) / denominator).max(0.);
                let cov = (ab - a * b / n) / denominator;
                let ssim = ((2. * ma * mb + 0.0001) * (2. * cov + 0.0009))
                    / ((ma * ma + mb * mb + 0.0001) * (va + vb + 0.0009));
                sum += (1. - ssim) * n;
                weight += n;
            }
        }
    }
    let score = (sum / weight).max(0.);
    let mut diff = RgbaImage::new(actual.width(), actual.height());
    for ((_, _, out), a) in diff
        .enumerate_pixels_mut()
        .zip(actual.pixels().zip(golden.pixels()))
    {
        *out = Rgba([
            a.0[0].abs_diff(a.1[0]).saturating_mul(4),
            a.0[1].abs_diff(a.1[1]).saturating_mul(4),
            a.0[2].abs_diff(a.1[2]).saturating_mul(4),
            255,
        ]);
    }
    Ok(Comparison {
        score,
        passed: score <= tolerance,
        diff,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssim_detects_changes_and_emits_diff() {
        let a = RgbaImage::from_pixel(17, 11, Rgba([100, 120, 140, 255]));
        assert_eq!(compare(&a, &a, 0.).unwrap().score, 0.);
        let b = RgbaImage::from_pixel(17, 11, Rgba([200, 10, 50, 255]));
        let result = compare(&a, &b, 0.02).unwrap();
        assert!(!result.passed);
        assert_eq!(result.diff.get_pixel(0, 0)[0], 255);
        assert!(compare(&a, &RgbaImage::new(1, 1), 0.01).is_err());
    }
}
