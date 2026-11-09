//! Fits the linearly transformed cosine (LTC) tables the raster renderer's area lights
//! read (`look::ltc`, `shaders/look-area.slangh`), and writes `data/ltc.bin`.
//!
//! `cargo run -p ara-render --release --example ltc_fit` (about a minute). The output is
//! checked in; rerun only when Ara's specular lobe changes.
//!
//! Method: Heitz, Dupuy, Hill and Neubelt 2016, "Real-Time Polygonal-Light Shading
//! with Linearly Transformed Cosines", §4 (an independent implementation, not their
//! code). For each perceptual roughness r (alpha = r^2) and view angle (indexed by
//! sqrt(1 - cos theta)), the clamped-cosine distribution transformed by
//! M = [X Y Z] [[m00, 0, m02], [0, m11, 0], [0, 0, 1]] is fitted by Nelder-Mead to
//! Ara's cosine-weighted specular lobe: GGX D with height-correlated Smith masking
//! (Heitz 2014), no Fresnel, normalised by its albedo. The error is the cubed
//! difference of the two distributions, importance-sampled from both (balance MIS).
//! The renderer scales the normalised integral by its own compensated albedo
//! (`Lobes::albedo`), so Fresnel and multiple scattering stay those of point lights.
//!
//! Table 1 stores M^-1 / M^-1[1][1] in the shading frame (tangent towards the
//! viewer, bitangent, normal) as (m00, m02, m20, m22); the other entries are 0
//! and 1. Table 2 is the horizon-clipped form factor of a spherical cap (the
//! sphere approximation of Heitz and Hill 2017, "Real-time line- and disk-light
//! shading with linearly transformed cosines"): for a cap whose vector form factor
//! has length `len` and elevation cosine `z`, clipped form factor / len. It is
//! integrated numerically here rather than by Snyder's closed form.
use glam::{Mat3, Vec3};
use std::f32::consts::PI;

/// Table side (both axes).
const N: usize = 64;
/// Samples per axis in the fit error and the albedo terms.
const SAMPLES: usize = 32;
/// Smallest alpha fitted (perceptual roughness 0 maps here).
const MIN_ALPHA: f32 = 1e-4;

/// Ara's cosine-weighted specular lobe: D G2 / (4 n.v) without Fresnel, and the
/// pdf of sampling L through GGX half vectors.
fn brdf(v: Vec3, l: Vec3, alpha: f32) -> (f32, f32) {
    if v.z <= 0. || l.z <= 0. {
        return (0., 0.);
    }
    let h = (v + l).normalize();
    let a2 = alpha * alpha;
    // n.h^2 (a2 - 1) + 1 without cancellation: a nearly mirror lobe stays finite.
    let t = h.z * h.z * a2 + (h.x * h.x + h.y * h.y);
    let d = a2 / (PI * t * t);
    let lambda = |w: Vec3| {
        let tan2 = (1. - w.z * w.z).max(0.) / (w.z * w.z);
        (-1. + (1. + a2 * tan2).sqrt()) * 0.5
    };
    let g2 = 1. / (1. + lambda(v) + lambda(l));
    let value = d * g2 / (4. * v.z);
    let pdf = d * h.z / (4. * v.dot(h).max(1e-7));
    (value, pdf)
}

/// A GGX half-vector sample reflected about `v`.
fn sample_brdf(v: Vec3, alpha: f32, u1: f32, u2: f32) -> Vec3 {
    let phi = 2. * PI * u1;
    let cos2 = (1. - u2) / (1. + (alpha * alpha - 1.) * u2);
    let (cos, sin) = (cos2.sqrt(), (1. - cos2).max(0.).sqrt());
    let h = Vec3::new(sin * phi.cos(), sin * phi.sin(), cos);
    (2. * v.dot(h) * h - v).normalize()
}

/// A clamped cosine distribution transformed by `m` (normalised; `magnitude` scales it).
#[derive(Clone, Copy)]
struct Ltc {
    m00: f32,
    m11: f32,
    m02: f32,
    m: Mat3,
    inverse: Mat3,
    det: f32,
}

impl Ltc {
    fn new(frame: Mat3, m00: f32, m11: f32, m02: f32) -> Self {
        let m = frame
            * Mat3::from_cols(
                Vec3::new(m00, 0., 0.),
                Vec3::new(0., m11, 0.),
                Vec3::new(m02, 0., 1.),
            );
        Self {
            m00,
            m11,
            m02,
            m,
            inverse: m.inverse(),
            det: m.determinant().abs(),
        }
    }

    fn eval(&self, l: Vec3) -> f32 {
        let original = (self.inverse * l).normalize();
        let transformed = self.m * original;
        let length = transformed.length();
        let jacobian = self.det / (length * length * length);
        original.z.max(0.) / PI / jacobian
    }

    fn sample(&self, u1: f32, u2: f32) -> Vec3 {
        let (cos, sin) = ((1. - u1).sqrt(), u1.sqrt());
        let phi = 2. * PI * u2;
        (self.m * Vec3::new(sin * phi.cos(), sin * phi.sin(), cos)).normalize()
    }
}

/// The lobe's albedo and its albedo-weighted mean direction (projected into xz).
fn moments(v: Vec3, alpha: f32) -> (f32, Vec3) {
    let (mut albedo, mut direction) = (0., Vec3::ZERO);
    for j in 0..SAMPLES {
        for i in 0..SAMPLES {
            let (u1, u2) = (
                (i as f32 + 0.5) / SAMPLES as f32,
                (j as f32 + 0.5) / SAMPLES as f32,
            );
            let l = sample_brdf(v, alpha, u1, u2);
            let (value, pdf) = brdf(v, l, alpha);
            if pdf > 0. {
                albedo += value / pdf;
                direction += l * (value / pdf);
            }
        }
    }
    let count = (SAMPLES * SAMPLES) as f32;
    direction.y = 0.;
    (albedo / count, direction.normalize_or(Vec3::Z))
}

/// Fit error of `ltc` against the lobe normalised by `albedo`.
fn error(ltc: &Ltc, v: Vec3, alpha: f32, albedo: f32) -> f64 {
    let mut sum = 0f64;
    let term = |l: Vec3| {
        let (value, pdf_brdf) = brdf(v, l, alpha);
        let (value, pdf_brdf) = (value / albedo, pdf_brdf);
        let fitted = ltc.eval(l);
        let difference = f64::from((value - fitted).abs());
        difference.powi(3) / f64::from(fitted + pdf_brdf).max(1e-12)
    };
    for j in 0..SAMPLES {
        for i in 0..SAMPLES {
            let (u1, u2) = (
                (i as f32 + 0.5) / SAMPLES as f32,
                (j as f32 + 0.5) / SAMPLES as f32,
            );
            sum += term(ltc.sample(u1, u2));
            sum += term(sample_brdf(v, alpha, u1, u2));
        }
    }
    sum / (SAMPLES * SAMPLES) as f64
}

/// Nelder-Mead over three parameters from `start`, with an initial simplex of
/// `delta` along each axis.
fn minimise(start: [f32; 3], delta: f32, mut f: impl FnMut([f32; 3]) -> f64) -> [f32; 3] {
    let mut points = [start; 4];
    for (i, point) in points.iter_mut().enumerate().skip(1) {
        point[i - 1] += delta;
    }
    let mut values = points.map(&mut f);
    let along = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
    for _ in 0..200 {
        let mut order = [0, 1, 2, 3];
        order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
        points = order.map(|i| points[i]);
        values = order.map(|i| values[i]);
        if (values[3] - values[0]).abs() < 1e-7 * values[0].abs().max(1e-12) {
            break;
        }
        let centroid = [0, 1, 2].map(|k| (points[0][k] + points[1][k] + points[2][k]) / 3.);
        let reflected = along(centroid, points[3], -1.);
        let fr = f(reflected);
        if fr < values[0] {
            let expanded = along(centroid, points[3], -2.);
            let fe = f(expanded);
            (points[3], values[3]) = if fe < fr {
                (expanded, fe)
            } else {
                (reflected, fr)
            };
        } else if fr < values[2] {
            (points[3], values[3]) = (reflected, fr);
        } else {
            let contracted = if fr < values[3] {
                along(centroid, reflected, 0.5)
            } else {
                along(centroid, points[3], 0.5)
            };
            let fc = f(contracted);
            if fc < values[3].min(fr) {
                (points[3], values[3]) = (contracted, fc);
            } else {
                for i in 1..4 {
                    points[i] = along(points[0], points[i], 0.5);
                    values[i] = f(points[i]);
                }
            }
        }
    }
    let best = (0..4).min_by(|&a, &b| values[a].total_cmp(&values[b]));
    points[best.unwrap_or(0)]
}

/// View direction for table column `t`: cos theta = 1 - x^2, x = t / (N - 1).
fn view(t: usize) -> Vec3 {
    let x = t as f32 / (N - 1) as f32;
    let theta = (1. - x * x).clamp(0., 1.).acos().min(1.57);
    Vec3::new(theta.sin(), 0., theta.cos())
}

fn alpha(a: usize) -> f32 {
    let r = a as f32 / (N - 1) as f32;
    (r * r).max(MIN_ALPHA)
}

/// Fits one cell starting from `guess`'s parameters, oriented by the lobe's mean.
fn fit(a: usize, t: usize, guess: Option<Ltc>) -> (Ltc, f64) {
    let (v, alpha) = (view(t), alpha(a));
    let (albedo, direction) = moments(v, alpha);
    let isotropic = t == 0;
    let frame = if isotropic {
        Mat3::IDENTITY
    } else {
        Mat3::from_cols(Vec3::new(direction.z, 0., -direction.x), Vec3::Y, direction)
    };
    let start = guess.map_or([1., 1., 0.], |g| [g.m00, g.m11, g.m02]);
    let build = |p: [f32; 3]| {
        let (m00, m11) = (p[0].max(1e-7), p[1].max(1e-7));
        if isotropic {
            Ltc::new(frame, m00, m00, 0.)
        } else {
            Ltc::new(frame, m00, m11, p[2])
        }
    };
    // A restart from the first optimum escapes the simplex collapsing early.
    let first = minimise(start, 0.05, |p| error(&build(p), v, alpha, albedo));
    let best = minimise(first, 0.01, |p| error(&build(p), v, alpha, albedo));
    let ltc = build(best);
    (ltc, error(&ltc, v, alpha, albedo))
}

/// Projected solid angle / pi of a spherical cap of half-angle `sigma` whose centre
/// lies `omega` from the zenith, clipped to the upper hemisphere (midpoint rule).
fn clipped_cap(omega: f32, sigma: f32) -> f64 {
    const STEPS: usize = 384;
    let centre = Vec3::new(omega.sin(), 0., omega.cos());
    let (tangent, bitangent) = (Vec3::new(omega.cos(), 0., -omega.sin()), Vec3::Y);
    let mut sum = 0f64;
    for i in 0..STEPS {
        let beta = (i as f32 + 0.5) / STEPS as f32 * sigma;
        let ring = f64::from(beta.sin()) * f64::from(sigma / STEPS as f32);
        for j in 0..STEPS {
            let gamma = (j as f32 + 0.5) / STEPS as f32 * 2. * PI;
            let d = centre * beta.cos()
                + (tangent * gamma.cos() + bitangent * gamma.sin()) * beta.sin();
            sum += f64::from(d.z.max(0.)) * ring;
        }
    }
    sum * f64::from(2. * PI / STEPS as f32) / std::f64::consts::PI
}

/// Round-to-nearest-even f32 to IEEE half bits (finite inputs).
fn half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent >= 31 {
        return sign | 0x7bff;
    }
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let m = (mantissa | 0x80_0000) >> (1 - exponent);
        let rounded = (m + 0xfff + ((m >> 13) & 1)) >> 13;
        return sign | rounded as u16;
    }
    let rounded = ((exponent as u32) << 10 | mantissa >> 13) + {
        let rest = mantissa & 0x1fff;
        u32::from(rest > 0x1000 || (rest == 0x1000 && (mantissa >> 13) & 1 == 1))
    };
    sign | rounded.min(0x7bff) as u16
}

fn main() {
    // Diagnostics: ARA_LTC_PROBE=a,t fits one cell from a cold start and reports it.
    if let Ok(probe) = std::env::var("ARA_LTC_PROBE") {
        let mut parts = probe
            .split(',')
            .map(|p| p.trim().parse::<usize>().expect("a,t"));
        let (a, t) = (parts.next().expect("a"), parts.next().expect("t"));
        let (v, alpha) = (view(t), alpha(a));
        let (albedo, direction) = moments(v, alpha);
        println!(
            "alpha {alpha} v {v} albedo {albedo} mean direction {direction} mirror {}",
            Vec3::new(-v.x, 0., v.z)
        );
        for guess in [None, Some(Ltc::new(Mat3::IDENTITY, alpha, alpha, 0.))] {
            let (ltc, e) = fit(a, t, guess);
            let peak = (ltc.m * Vec3::Z).normalize();
            println!(
                "m00 {} m11 {} m02 {} error {e:.3e} peak {peak}",
                ltc.m00, ltc.m11, ltc.m02
            );
        }
        return;
    }
    // Column t = 0 (normal view) first, from rough to smooth, each from its neighbour;
    // then every roughness row in parallel along increasing view angle.
    let mut column: Vec<Option<Ltc>> = vec![None; N];
    for a in (0..N).rev() {
        column[a] = Some(fit(a, 0, column.get(a + 1).copied().flatten()).0);
    }
    let rows: Vec<Vec<(Ltc, f64)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..N)
            .map(|a| {
                let first = column[a];
                scope.spawn(move || {
                    let mut row = Vec::with_capacity(N);
                    let mut previous = first;
                    for t in 0..N {
                        let cell = if t == 0 {
                            let ltc = first.expect("column fitted");
                            let (v, alpha) = (view(0), alpha(a));
                            let albedo = moments(v, alpha).0;
                            (ltc, error(&ltc, v, alpha, albedo))
                        } else {
                            // The warm start (the previous view angle's fit) can drift
                            // into a local minimum for narrow lobes: also start cold
                            // (isotropic alpha, no shear) and keep the better fit.
                            let warm = fit(a, t, previous);
                            let alpha = alpha(a);
                            let cold = fit(a, t, Some(Ltc::new(Mat3::IDENTITY, alpha, alpha, 0.)));
                            if cold.1 < warm.1 { cold } else { warm }
                        };
                        previous = Some(cell.0);
                        row.push(cell);
                    }
                    row
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("fit thread"))
            .collect()
    });
    let mut matrices = Vec::with_capacity(N * N * 4);
    let (mut worst, mut mean) = (0f64, 0f64);
    // Row-major by view index (texture y), roughness along x.
    for t in 0..N {
        for row in &rows {
            let (ltc, e) = row[t];
            if !e.is_finite() || !ltc.m.is_finite() {
                eprintln!(
                    "cell r{} t{t}: error {e}, m00 {} m11 {} m02 {}",
                    rows.iter().position(|r| std::ptr::eq(r, row)).unwrap_or(0),
                    ltc.m00,
                    ltc.m11,
                    ltc.m02
                );
            }
            worst = worst.max(e);
            mean += e / (N * N) as f64;
            let inverse = ltc.m.inverse();
            let scale = inverse.y_axis.y;
            let m = inverse * (1. / scale);
            // Row-major (m00, m02, m20, m22): glam is column-major, m[col][row].
            matrices.extend([m.x_axis.x, m.z_axis.x, m.x_axis.z, m.z_axis.z].map(half));
        }
    }
    let mut sphere = Vec::with_capacity(N * N);
    for j in 0..N {
        for i in 0..N {
            let z = 2. * i as f32 / (N - 1) as f32 - 1.;
            let len = j as f32 / (N - 1) as f32;
            let value = if len > 0. {
                let sigma = len.sqrt().min(1.).asin();
                (clipped_cap(z.clamp(-1., 1.).acos(), sigma) / f64::from(len)) as f32
            } else {
                z.max(0.)
            };
            sphere.push(half(value));
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"ARALTC01");
    out.extend_from_slice(&(N as u32).to_le_bytes());
    for v in matrices.iter().chain(&sphere) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/ltc.bin");
    std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/data")).expect("data dir");
    std::fs::write(path, out).expect("write ltc.bin");
    println!("wrote {path}: mean fit error {mean:.3e}, worst {worst:.3e}");
}
