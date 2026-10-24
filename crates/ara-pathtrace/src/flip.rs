//! LDR ꟻLIP (Andersson et al. 2020, "FLIP: A Difference Evaluator for Alternating
//! Images"), an independent implementation of the published algorithm: CSF-filtered
//! YCxCz colour difference (Hunt-adjusted HyAB) weighted by edge and point feature
//! differences. Use it to compare display-referred captures with a reference at a
//! stated viewing condition; 0 is identical, 1 is maximal.
use anyhow::{Result, ensure};
use image::RgbaImage;

/// Default viewing condition: a 0.7 m distance to a 0.7 m wide 3840-pixel monitor
/// (the paper's 67 pixels per degree).
pub const PIXELS_PER_DEGREE: f32 = 67.0;

/// Mean and per-pixel ꟻLIP error of `test` against `reference`.
pub struct FlipResult {
    /// Mean error over every pixel.
    pub mean: f64,
    /// Per-pixel error, row-major.
    pub errors: Vec<f32>,
}

/// LDR ꟻLIP of two equally sized sRGB images at `ppd` pixels per degree.
pub fn flip(reference: &RgbaImage, test: &RgbaImage, ppd: f32) -> Result<FlipResult> {
    ensure!(
        reference.dimensions() == test.dimensions() && reference.width() > 0,
        "FLIP images differ in size"
    );
    let (w, h) = (reference.width() as usize, reference.height() as usize);
    let ycxcz = |img: &RgbaImage| -> [Vec<f32>; 3] {
        let mut out = [vec![0.; w * h], vec![0.; w * h], vec![0.; w * h]];
        for (i, p) in img.pixels().enumerate() {
            let rgb = [0, 1, 2].map(|c| srgb_to_linear(f32::from(p[c]) / 255.));
            let c = linear_to_ycxcz(rgb);
            for k in 0..3 {
                out[k][i] = c[k];
            }
        }
        out
    };
    let (yr, yt) = (ycxcz(reference), ycxcz(test));
    // Contrast sensitivity: achromatic, red-green, blue-yellow (a1, b1, a2, b2).
    let csf = [
        [1.0, 0.0047, 0.0, 1e-5],
        [1.0, 0.0053, 0.0, 1e-5],
        [34.1, 0.04, 13.5, 0.025],
    ];
    let max_b = 0.04f32;
    let radius = (3.0 * (max_b / (2.0 * std::f32::consts::PI.powi(2))).sqrt() * ppd).ceil() as i32;
    let filtered = |c: &[Vec<f32>; 3]| -> Vec<[f32; 3]> {
        let mut channels = Vec::new();
        for (k, [a1, b1, a2, b2]) in csf.iter().enumerate() {
            // The 2D CSF is a sum of two radial Gaussians; each is separable, so
            // filter them separately and normalize by the 2D kernel's total weight.
            let mut sum = vec![0.; w * h];
            let mut weight = 0.;
            for (a, b) in [(*a1, *b1), (*a2, *b2)] {
                if a == 0. {
                    continue;
                }
                let profile: Vec<f32> = (-radius..=radius)
                    .map(|i| {
                        let x = i as f32 / ppd;
                        (-std::f32::consts::PI.powi(2) * x * x / b).exp()
                    })
                    .collect();
                let scale = a * (std::f32::consts::PI / b).sqrt();
                let total: f32 = profile.iter().sum();
                weight += scale * total * total;
                for (s, v) in sum
                    .iter_mut()
                    .zip(separable(&c[k], w, h, &profile, &profile))
                {
                    *s += scale * v;
                }
            }
            channels.push(sum.into_iter().map(|v| v / weight).collect::<Vec<f32>>());
        }
        (0..w * h)
            .map(|i| {
                let rgb = ycxcz_to_linear([channels[0][i], channels[1][i], channels[2][i]]);
                hunt(linear_to_lab(rgb.map(|v| v.clamp(0., 1.))))
            })
            .collect()
    };
    let (fr, ft) = (filtered(&yr), filtered(&yt));
    let green = hunt(linear_to_lab([0., 1., 0.]));
    let blue = hunt(linear_to_lab([0., 0., 1.]));
    let cmax = hyab(green, blue).powf(0.7);
    let (pc, pt) = (0.4f32, 0.95f32);
    // Feature detectors on normalized achromatic luminance.
    let sigma = 0.5 * 0.082 * ppd;
    let fr_radius = (3.0 * sigma).ceil() as i32;
    let gauss: Vec<f32> = (-fr_radius..=fr_radius)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let normalize = |k: Vec<f32>| -> Vec<f32> {
        let pos: f32 = k.iter().filter(|v| **v > 0.).sum();
        let neg: f32 = -k.iter().filter(|v| **v < 0.).sum::<f32>();
        k.iter()
            .map(|v| {
                if *v > 0. {
                    v / pos
                } else if neg > 0. {
                    v / neg
                } else {
                    *v
                }
            })
            .collect()
    };
    let gsum: f32 = gauss.iter().sum();
    let smooth: Vec<f32> = gauss.iter().map(|v| v / gsum).collect();
    let edge = normalize(
        (-fr_radius..=fr_radius)
            .zip(&gauss)
            .map(|(i, g)| -(i as f32) * g)
            .collect(),
    );
    let point = normalize(
        (-fr_radius..=fr_radius)
            .zip(&gauss)
            .map(|(i, g)| ((i * i) as f32 / (sigma * sigma) - 1.0) * g)
            .collect(),
    );
    let features = |c: &[Vec<f32>; 3]| -> (Vec<f32>, Vec<f32>) {
        let l: Vec<f32> = c[0].iter().map(|y| (y + 16.0) / 116.0).collect();
        let ex = separable(&l, w, h, &edge, &smooth);
        let ey = separable(&l, w, h, &smooth, &edge);
        let px = separable(&l, w, h, &point, &smooth);
        let py = separable(&l, w, h, &smooth, &point);
        (
            ex.iter().zip(&ey).map(|(a, b)| a.hypot(*b)).collect(),
            px.iter().zip(&py).map(|(a, b)| a.hypot(*b)).collect(),
        )
    };
    let ((er, pr), (et, ptest)) = (features(&yr), features(&yt));
    let mut errors = Vec::with_capacity(w * h);
    let mut total = 0.0f64;
    for i in 0..w * h {
        let de = hyab(fr[i], ft[i]).powf(0.7);
        let colour = if de < pc * cmax {
            pt / (pc * cmax) * de
        } else {
            pt + (de - pc * cmax) / (cmax - pc * cmax) * (1.0 - pt)
        };
        let df = (er[i] - et[i]).abs().max((pr[i] - ptest[i]).abs());
        let feature = (std::f32::consts::FRAC_1_SQRT_2 * df).powf(0.5);
        let e = colour.min(1.0).powf(1.0 - feature);
        total += f64::from(e);
        errors.push(e);
    }
    Ok(FlipResult {
        mean: total / (w * h) as f64,
        errors,
    })
}

/// Horizontal then vertical convolution with clamped borders.
fn separable(src: &[f32], w: usize, h: usize, kx: &[f32], ky: &[f32]) -> Vec<f32> {
    let (rx, ry) = ((kx.len() / 2) as isize, (ky.len() / 2) as isize);
    let mut tmp = vec![0.; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.;
            for (j, k) in kx.iter().enumerate() {
                let xx = (x as isize + j as isize - rx).clamp(0, w as isize - 1) as usize;
                s += src[y * w + xx] * k;
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.;
            for (j, k) in ky.iter().enumerate() {
                let yy = (y as isize + j as isize - ry).clamp(0, h as isize - 1) as usize;
                s += tmp[yy * w + x] * k;
            }
            out[y * w + x] = s;
        }
    }
    out
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

const WHITE: [f32; 3] = [0.950_428_5, 1.0, 1.088_9];

fn linear_to_xyz(c: [f32; 3]) -> [f32; 3] {
    [
        0.412_390_8 * c[0] + 0.357_584_3 * c[1] + 0.180_480_8 * c[2],
        0.212_639 * c[0] + 0.715_168_7 * c[1] + 0.072_192_3 * c[2],
        0.019_330_8 * c[0] + 0.119_194_8 * c[1] + 0.950_532_2 * c[2],
    ]
}

fn xyz_to_linear(c: [f32; 3]) -> [f32; 3] {
    [
        3.240_97 * c[0] - 1.537_383 * c[1] - 0.498_611 * c[2],
        -0.969_243_6 * c[0] + 1.875_967_5 * c[1] + 0.041_555_1 * c[2],
        0.055_63 * c[0] - 0.203_977 * c[1] + 1.056_971_5 * c[2],
    ]
}

fn linear_to_ycxcz(c: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = linear_to_xyz(c);
    let [xn, yn, zn] = [x / WHITE[0], y / WHITE[1], z / WHITE[2]];
    [116.0 * yn - 16.0, 500.0 * (xn - yn), 200.0 * (yn - zn)]
}

fn ycxcz_to_linear(c: [f32; 3]) -> [f32; 3] {
    let yn = (c[0] + 16.0) / 116.0;
    let xn = c[1] / 500.0 + yn;
    let zn = yn - c[2] / 200.0;
    xyz_to_linear([xn * WHITE[0], yn * WHITE[1], zn * WHITE[2]])
}

fn linear_to_lab(c: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = linear_to_xyz(c);
    let f = |t: f32| {
        let d = 6.0f32 / 29.0;
        if t > d * d * d {
            t.cbrt()
        } else {
            t / (3.0 * d * d) + 4.0 / 29.0
        }
    };
    let (fx, fy, fz) = (f(x / WHITE[0]), f(y / WHITE[1]), f(z / WHITE[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Hunt effect: chroma scales with lightness.
fn hunt(lab: [f32; 3]) -> [f32; 3] {
    [lab[0], 0.01 * lab[0] * lab[1], 0.01 * lab[0] * lab[2]]
}

fn hyab(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).hypot(a[2] - b[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_have_zero_error_and_opposites_are_large() {
        let mut a = RgbaImage::new(48, 32);
        for (x, y, p) in a.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 5) as u8, (y * 7) as u8, 128, 255]);
        }
        assert!(flip(&a, &a, PIXELS_PER_DEGREE).unwrap().mean < 1e-6);
        let black = RgbaImage::from_pixel(48, 32, image::Rgba([0, 0, 0, 255]));
        let white = RgbaImage::from_pixel(48, 32, image::Rgba([255, 255, 255, 255]));
        let e = flip(&black, &white, PIXELS_PER_DEGREE).unwrap().mean;
        assert!(e > 0.9, "{e}");
        let grey = RgbaImage::from_pixel(48, 32, image::Rgba([128, 128, 128, 255]));
        let near = RgbaImage::from_pixel(48, 32, image::Rgba([132, 128, 128, 255]));
        let small = flip(&grey, &near, PIXELS_PER_DEGREE).unwrap().mean;
        assert!(small > 0. && small < 0.1, "{small}");
    }
}
