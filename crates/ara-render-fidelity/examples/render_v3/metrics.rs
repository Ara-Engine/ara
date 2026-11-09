//! Quality is evaluated on the displayed capped frame, never on a settled replacement.
use super::contract::Quality;
pub type LinearPair<'a> = (&'a [[f32; 4]], &'a [[f32; 4]]);
use anyhow::{Result, ensure};

pub fn compare(
    size: [u32; 2],
    test: &[[f32; 4]],
    reference: &[[f32; 4]],
    white: f64,
    previous: Option<LinearPair<'_>>,
) -> Result<Quality> {
    let count = size[0] as usize * size[1] as usize;
    ensure!(
        white.is_finite() && white > 0. && test.len() == count && reference.len() == count,
        "invalid linear comparison"
    );
    ensure!(
        test.iter()
            .chain(reference)
            .all(|p| p[..3].iter().all(|v| v.is_finite())),
        "nonfinite radiance"
    );
    let mut result = Quality::default();
    if let Some((pt, pr)) = previous {
        ensure!(
            pt.len() == count && pr.len() == count,
            "temporal extent mismatch"
        );
        let lum = |p: [f32; 4]| {
            0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
        };
        for by in (0..size[1]).step_by(16) {
            for bx in (0..size[0]).step_by(16) {
                let mut sum = 0.;
                let mut n = 0;
                for y in by..(by + 16).min(size[1]) {
                    for x in bx..(bx + 16).min(size[0]) {
                        let i = (y * size[0] + x) as usize;
                        let delta = ((lum(test[i]) - lum(pt[i]))
                            - (lum(reference[i]) - lum(pr[i])))
                            / white;
                        sum += delta * delta;
                        n += 1;
                    }
                }
                result.temporal_blocks.push((sum / f64::from(n)).sqrt());
            }
        }
        result.temporal_block_rms_white = result.temporal_blocks.iter().copied().reduce(f64::max);
    }
    Ok(result)
}

pub fn regions(errors: &[f32], size: [u32; 2]) -> std::collections::BTreeMap<String, f64> {
    let mut regions = std::collections::BTreeMap::new();
    // Whole image and a fixed 4x4 partition: each case records these exact regions.
    regions.insert(
        "whole".into(),
        errors.iter().map(|v| f64::from(*v)).sum::<f64>() / errors.len() as f64,
    );
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0.;
            let mut n = 0;
            for y in row * size[1] / 4..(row + 1) * size[1] / 4 {
                for x in col * size[0] / 4..(col + 1) * size[0] / 4 {
                    sum += f64::from(errors[(y * size[0] + x) as usize]);
                    n += 1;
                }
            }
            if n > 0 {
                regions.insert(format!("r{row}c{col}"), sum / f64::from(n));
            }
        }
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moving_reference_is_subtracted_before_block_rms() {
        let a = vec![[0.; 4]; 32 * 17];
        let b = vec![[0.5; 4]; 32 * 17];
        assert_eq!(
            compare([32, 17], &b, &b, 1., Some((&a, &a)))
                .unwrap()
                .temporal_block_rms_white,
            Some(0.)
        );
        let mut noise = b.clone();
        noise[32 * 16][0] += 0.1;
        let q = compare([32, 17], &noise, &b, 1., Some((&a, &a))).unwrap();
        assert_eq!(q.temporal_blocks.len(), 4);
        assert!((q.temporal_blocks[2] - 0.02126 / 4.).abs() < 1e-7);
    }
}
