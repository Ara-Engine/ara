//! Metrics in linear light and over ordered, independently captured sequences.
use anyhow::{Result, ensure};
use serde::Serialize;
/// Radiance error including an explicit reference energy denominator.
#[derive(Debug, Serialize)]
pub struct RadianceError {
    /// Root mean squared absolute RGB radiance difference.
    pub rmse: f64,
    /// RMSE divided by RMS reference radiance; zero-energy images use 1e-6.
    pub relative_rmse: f64,
    /// Mean absolute radiance error, useful near black.
    pub mae: f64,
}
/// Compare full-precision linear transport; NaNs always fail validation.
pub fn radiance(actual: &[[f32; 4]], reference: &[[f32; 4]]) -> Result<RadianceError> {
    ensure!(
        !actual.is_empty() && actual.len() == reference.len(),
        "Radiance dimensions differ"
    );
    let mut square = 0.;
    let mut energy = 0.;
    let mut absolute = 0.;
    for (a, b) in actual.iter().zip(reference) {
        for c in 0..3 {
            ensure!(a[c].is_finite() && b[c].is_finite(), "Nonfinite radiance");
            let d = f64::from(a[c]) - f64::from(b[c]);
            square += d * d;
            absolute += d.abs();
            energy += f64::from(b[c]).powi(2);
        }
    }
    let n = (actual.len() * 3) as f64;
    let rmse = (square / n).sqrt();
    Ok(RadianceError {
        rmse,
        relative_rmse: rmse / (energy / n).sqrt().max(1e-6),
        mae: absolute / n,
    })
}
/// Excess temporal residual relative to a history-free frame. A static image with
/// high similarity does not prove this metric; compare moving/disoccluded regions.
pub fn temporal_residual(
    accumulated: &image::RgbaImage,
    fresh: &image::RgbaImage,
    mask: impl Fn(u32, u32) -> bool,
) -> Result<f64> {
    ensure!(
        accumulated.dimensions() == fresh.dimensions(),
        "Sequence dimensions differ"
    );
    let mut sum = 0.;
    let mut count = 0;
    for (x, y, a) in accumulated.enumerate_pixels() {
        if mask(x, y) {
            let b = fresh.get_pixel(x, y);
            for i in 0..3 {
                sum += f64::from(a[i].abs_diff(b[i])) / 255.;
                count += 1;
            }
        }
    }
    ensure!(count > 0, "Empty temporal mask");
    Ok(sum / f64::from(count))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radiance_reports_known_error_and_rejects_nan() {
        let a = [[1., 1., 1., 1.]];
        let b = [[2., 2., 2., 1.]];
        let e = radiance(&a, &b).unwrap();
        assert_eq!(e.rmse, 1.);
        assert_eq!(e.relative_rmse, 0.5);
        assert!(radiance(&[[f32::NAN; 4]], &a).is_err());
    }
}
