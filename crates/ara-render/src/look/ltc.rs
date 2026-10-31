//! Linearly transformed cosine tables for area lights (`shaders/look-lights.slangh`):
//! fitted to Ara's GGX lobe by `examples/ltc_fit.rs` (provenance and method there) and
//! checked in as `data/ltc.bin`. [`LtcTables::new`] uploads them once per renderer as
//! two sampled images the look record names (`LookFrame::light_info[0]`).
//!
//! `data/ltc.bin`: `ARALTC01`, the side `N` (u32 LE), then IEEE half floats: the
//! RGBA inverse matrices, N x N row-major (x: perceptual roughness, y: sqrt(1 - n.v)),
//! then the N x N horizon-clipped sphere table (x: z * 0.5 + 0.5, y: form-factor length).
use anyhow::{Result, ensure};
use ara_gpu::{Device, GpuDevice, Image, Resource, vk};

/// The fitted tables.
const DATA: &[u8] = include_bytes!("../../data/ltc.bin");
/// Header bytes before the tables.
const HEADER: usize = 12;

/// Table side, from the header.
fn side() -> usize {
    u32::from_le_bytes([DATA[8], DATA[9], DATA[10], DATA[11]]) as usize
}

/// The RGBA16F inverse-matrix table and the R16F sphere table, as uploaded.
fn tables() -> Result<(&'static [u8], &'static [u8])> {
    ensure!(
        DATA.len() >= HEADER && &DATA[..8] == b"ARALTC01",
        "Invalid LTC tables"
    );
    let n = side();
    let matrices = n * n * 4 * 2;
    ensure!(
        DATA.len() == HEADER + matrices + n * n * 2,
        "Truncated LTC tables"
    );
    Ok((&DATA[HEADER..HEADER + matrices], &DATA[HEADER + matrices..]))
}

/// The tables on the GPU for the renderer's lifetime (copied into secondary views).
#[derive(Clone, Copy)]
pub(crate) struct LtcTables {
    images: [Image; 2],
    /// Sampled indices: inverse matrices, sphere table.
    pub sampled: [u32; 2],
}

impl LtcTables {
    pub(crate) fn new(gpu: &mut GpuDevice) -> Result<Self> {
        let (matrices, sphere) = tables()?;
        let n = side() as u32;
        let first =
            crate::renderer::upload_image(gpu, [n, n], vk::Format::R16G16B16A16_SFLOAT, matrices)?;
        let second =
            match crate::renderer::upload_image(gpu, [n, n], vk::Format::R16_SFLOAT, sphere) {
                Ok(image) => image,
                Err(error) => {
                    gpu.retire(Resource::Image(first))?;
                    return Err(error);
                }
            };
        let indices = (|| -> Result<[u32; 2]> {
            Ok([
                gpu.sampled_image(first, false)?.index,
                gpu.sampled_image(second, false)?.index,
            ])
        })();
        match indices {
            Ok(sampled) => Ok(Self {
                images: [first, second],
                sampled,
            }),
            Err(error) => {
                gpu.retire(Resource::Image(first))?;
                gpu.retire(Resource::Image(second))?;
                Err(error)
            }
        }
    }

    /// The images, for graph imports (always `ShaderRead`) and retirement.
    pub(crate) fn resources(&self) -> [Resource; 2] {
        self.images.map(Resource::Image)
    }
}

#[cfg(test)]
mod tests {
    //! A CPU mirror of look-lights.slangh's LTC evaluation against brute-force
    //! integrals of the same GGX lobe: it checks the fit, the edge winding, the
    //! ellipse form factor and the horizon table together.
    use super::*;
    use glam::{Mat3, Vec2, Vec3};
    use std::f32::consts::PI;

    fn half(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1. } else { 1. };
        let exponent = i32::from((bits >> 10) & 0x1f);
        let mantissa = f32::from(bits & 0x3ff);
        sign * match exponent {
            0 => mantissa / 1024. * 2f32.powi(-14),
            _ => (1. + mantissa / 1024.) * 2f32.powi(exponent - 15),
        }
    }

    /// Bilinear clamp-to-edge lookup of channel `c` of a `channels`-wide table.
    fn lookup(table: &[u8], channels: usize, c: usize, uv: Vec2) -> f32 {
        let n = side();
        let at = |x: usize, y: usize| {
            let i = ((y * n + x) * channels + c) * 2;
            half(u16::from_le_bytes([table[i], table[i + 1]]))
        };
        let p = uv * n as f32 - 0.5;
        let (x0, y0) = (p.x.floor(), p.y.floor());
        let (tx, ty) = (p.x - x0, p.y - y0);
        let clamp = |v: f32| (v.max(0.) as usize).min(n - 1);
        let (xa, xb, ya, yb) = (clamp(x0), clamp(x0 + 1.), clamp(y0), clamp(y0 + 1.));
        let top = at(xa, ya) * (1. - tx) + at(xb, ya) * tx;
        let bottom = at(xa, yb) * (1. - tx) + at(xb, yb) * tx;
        top * (1. - ty) + bottom * ty
    }

    fn uv(x: f32, y: f32) -> Vec2 {
        let n = side() as f32;
        Vec2::new(x, y) * ((n - 1.) / n) + 0.5 / n
    }

    /// M^-1 as the shader builds it (row-major rows).
    fn inverse(roughness: f32, nv: f32) -> Mat3 {
        let (m, _) = tables().unwrap();
        let t = uv(roughness, (1. - nv).sqrt());
        let [a, b, c, d] = [0, 1, 2, 3].map(|i| lookup(m, 4, i, t));
        Mat3::from_cols(Vec3::new(a, 0., c), Vec3::Y, Vec3::new(b, 0., d))
    }

    fn clipped(f: Vec3) -> f32 {
        let (_, sphere) = tables().unwrap();
        let len = f.length();
        if len <= 1e-7 {
            return 0.;
        }
        len * lookup(sphere, 1, 0, uv(f.z / len * 0.5 + 0.5, len.min(1.)))
    }

    fn edge(a: Vec3, b: Vec3) -> Vec3 {
        let c = a.cross(b);
        let s = c.length();
        if s > 1e-7 {
            c * (s.atan2(a.dot(b)) / s)
        } else {
            Vec3::ZERO
        }
    }

    /// The shader's ltcEdgeClipped: edge a -> b (unnormalised) clipped to z >= 0; the
    /// horizon crossing becomes `exit` or `entry`.
    fn clipped_edge(a: Vec3, b: Vec3, exit: &mut Vec3, entry: &mut Vec3) -> f32 {
        if a.z < 0. && b.z < 0. {
            return 0.;
        }
        let cross = a + (b - a) * (a.z / (a.z - b.z));
        let (from, to) = if a.z < 0. {
            *entry = cross;
            (cross, b)
        } else if b.z < 0. {
            *exit = cross;
            (a, cross)
        } else {
            (a, b)
        };
        edge(from.normalize(), to.normalize()).z
    }

    /// Exact clamped-cosine integral of a convex polygon under `m`: each edge clipped
    /// at the horizon, closed by the horizon arc from the exit to the entry point.
    fn polygon(m: Mat3, corners: &[Vec3]) -> f32 {
        let p: Vec<Vec3> = corners.iter().map(|&c| m * c).collect();
        let (mut exit, mut entry) = (Vec3::ZERO, Vec3::ZERO);
        let mut sum = 0.;
        for i in 0..p.len() {
            sum += clipped_edge(p[i], p[(i + 1) % p.len()], &mut exit, &mut entry);
        }
        if exit != Vec3::ZERO && entry != Vec3::ZERO {
            sum += edge(exit.normalize(), entry.normalize()).z;
        }
        // Winding: corners counter-clockwise about a normal facing the origin
        // integrate negatively.
        let facing = (p[1] - p[0]).cross(p[2] - p[0]).dot(p[0]);
        (if facing < 0. { -sum } else { sum }).max(0.) / (2. * PI)
    }

    fn ellipse(m: Mat3, c: Vec3, u: Vec3, w: Vec3) -> f32 {
        let (c, u, w) = (m * c, m * u, m * w);
        let (d11, d22, d12) = (u.dot(u), w.dot(w), u.dot(w));
        let t = 0.5 * (2. * d12).atan2(d11 - d22);
        let (a1, a2) = (u * t.cos() + w * t.sin(), w * t.cos() - u * t.sin());
        let (e1, e2) = (a1.length(), a2.length());
        let (u1, u2) = (a1 / e1, a2 / e2);
        let mut u3 = u1.cross(u2).normalize();
        if u3.dot(c) < 0. {
            u3 = -u3;
        }
        let z = u3.dot(c);
        if z <= 1e-6 {
            return 0.;
        }
        let (x0, y0) = (u1.dot(c) / z, u2.dot(c) / z);
        let (a, b) = (z * z / (e1 * e1), z * z / (e2 * e2));
        let (q00, q11, q22, q02, q12) = (a, b, a * x0 * x0 + b * y0 * y0 - 1., -a * x0, -b * y0);
        let qm = (q00 + q11 + q22) / 3.;
        let p1 = q02 * q02 + q12 * q12;
        let p2 = (q00 - qm).powi(2) + (q11 - qm).powi(2) + (q22 - qm).powi(2) + 2. * p1;
        let p = (p2 / 6.).sqrt();
        let (b00, b11, b22, b02, b12) = (
            (q00 - qm) / p,
            (q11 - qm) / p,
            (q22 - qm) / p,
            q02 / p,
            q12 / p,
        );
        let r = ((b00 * (b11 * b22 - b12 * b12) - b11 * b02 * b02) * 0.5).clamp(-1., 1.);
        let phi = r.acos() / 3.;
        let e_max = qm + 2. * p * phi.cos();
        let e_min = qm + 2. * p * (phi + 2. * PI / 3.).cos();
        let e_mid = 3. * qm - e_max - e_min;
        let e_neg = -a * b / (e_max * e_mid);
        let form = -e_neg / ((e_max - e_neg) * (e_mid - e_neg)).sqrt();
        let axis = (u1 * (a * x0 / (a - e_neg)) + u2 * (b * y0 / (b - e_neg)) + u3).normalize();
        clipped(axis * form)
    }

    /// Ara's cosine-weighted lobe D G2 / (4 n.v), no Fresnel, in the frame where n = z.
    fn lobe(v: Vec3, l: Vec3, roughness: f32) -> f32 {
        if l.z <= 0. {
            return 0.;
        }
        let alpha = roughness * roughness;
        let a2 = alpha * alpha;
        let h = (v + l).normalize();
        let t = h.z * h.z * a2 + (h.x * h.x + h.y * h.y);
        let d = a2 / (PI * t * t);
        let lambda = |w: Vec3| (-1. + (1. + a2 * (1. - w.z * w.z) / (w.z * w.z)).sqrt()) * 0.5;
        d / (1. + lambda(v) + lambda(l)) / (4. * v.z)
    }

    /// Brute-force integral of the lobe over a parallelogram (centre, half axes) seen
    /// from the origin, and the lobe's albedo, in the shading frame.
    fn brute(v: Vec3, roughness: f32, centre: Vec3, x: Vec3, y: Vec3, disc: bool) -> (f32, f32) {
        let steps = 600;
        let normal = x.cross(y).normalize();
        let area = 4. * x.cross(y).length() / (steps * steps) as f32;
        let mut sum = 0.;
        for j in 0..steps {
            for i in 0..steps {
                let (s, t) = (
                    (i as f32 + 0.5) / steps as f32 * 2. - 1.,
                    (j as f32 + 0.5) / steps as f32 * 2. - 1.,
                );
                if disc && s * s + t * t > 1. {
                    continue;
                }
                let p = centre + x * s + y * t;
                let d2 = p.length_squared();
                let l = p / d2.sqrt();
                sum += lobe(v, l, roughness) * l.dot(normal).abs() / d2 * area;
            }
        }
        // The albedo by GGX half-vector sampling: a uniform grid misses narrow lobes.
        let (mut albedo, n) = (0., 400);
        let alpha = roughness * roughness;
        for j in 0..n {
            for i in 0..n {
                let (u1, u2) = ((i as f32 + 0.5) / n as f32, (j as f32 + 0.5) / n as f32);
                let cos2 = (1. - u2) / (1. + (alpha * alpha - 1.) * u2);
                let (c, s, phi) = (cos2.sqrt(), (1. - cos2).max(0.).sqrt(), 2. * PI * u1);
                let h = Vec3::new(s * phi.cos(), s * phi.sin(), c);
                let l = 2. * v.dot(h) * h - v;
                let a2 = alpha * alpha;
                let t = h.z * h.z * a2 + (h.x * h.x + h.y * h.y);
                let pdf = a2 / (PI * t * t) * h.z / (4. * v.dot(h).max(1e-7));
                if pdf > 0. {
                    albedo += lobe(v, l, roughness) / pdf / (n * n) as f32;
                }
            }
        }
        (sum, albedo)
    }

    #[test]
    fn ltc_matches_brute_force_ggx_over_rects_and_discs() {
        // (roughness, n.v, light centre, half axes, relative tolerance): overhead,
        // oblique, rough grazing, backscatter across the horizon (the three-parameter
        // fit overestimates it most) and a tilted mirror-like case.
        let cases = [
            (
                0.5f32,
                0.9f32,
                Vec3::new(0.3, 0.2, 1.5),
                Vec3::X * 0.5,
                Vec3::Y * 0.3,
                0.02,
            ),
            (
                0.3,
                0.6,
                Vec3::new(-1.2, 0.1, 1.0),
                Vec3::X * 0.4,
                Vec3::Y * 0.4,
                0.03,
            ),
            (
                0.8,
                0.4,
                Vec3::new(0.5, -0.4, 0.8),
                Vec3::X * 0.6,
                Vec3::Z * 0.3,
                0.1,
            ),
            (
                0.4,
                0.7,
                Vec3::new(-0.8, 0.0, 0.1),
                Vec3::Z * 0.4,
                Vec3::Y * 0.5,
                0.2,
            ),
            (
                0.2,
                0.8,
                Vec3::new(-1.0, 0.0, 1.0),
                (Vec3::X + Vec3::Z) * 0.25,
                Vec3::Y * 0.25,
                0.01,
            ),
            (
                4. / 63.,
                1. - (44f32 / 63.).powi(2),
                Vec3::new(-1.3, 0.0, 0.75),
                Vec3::new(0.5, 0., 0.866) * 0.3,
                Vec3::Y * 0.1,
                0.05,
            ),
            (
                0.3,
                0.5,
                Vec3::new(-1.3, 0.0, 0.75),
                Vec3::new(0.5, 0., 0.866) * 0.3,
                Vec3::Y * 0.1,
                0.05,
            ),
            (
                0.15,
                0.5,
                Vec3::new(-1.3, 0.0, 0.75),
                Vec3::new(0.5, 0., 0.866) * 0.3,
                Vec3::Y * 0.1,
                0.05,
            ),
            (
                0.07,
                0.5,
                Vec3::new(-1.3, 0.0, 0.75),
                Vec3::new(0.5, 0., 0.866) * 0.3,
                Vec3::Y * 0.1,
                0.05,
            ),
            (
                0.07,
                0.5,
                Vec3::new(-1.3, 0.15, 0.75),
                Vec3::new(0.5, 0., 0.866) * 0.3,
                Vec3::Y * 0.1,
                0.2,
            ),
            (
                0.1,
                0.25,
                Vec3::new(-1.45, 0.0, 0.37),
                Vec3::new(0.25, 0., 0.968) * 0.2,
                Vec3::Y * 0.2,
                0.05,
            ),
        ];
        for (roughness, nv, centre, x, y, tolerance) in cases {
            let v = Vec3::new((1. - nv * nv).sqrt(), 0., nv);
            let m = inverse(roughness, nv);
            for disc in [false, true] {
                let (exact, albedo) = brute(v, roughness, centre, x, y, disc);
                let fitted = albedo
                    * if disc {
                        ellipse(m, centre, x, y)
                    } else {
                        polygon(
                            m,
                            &[
                                centre - x - y,
                                centre + x - y,
                                centre + x + y,
                                centre - x + y,
                            ],
                        )
                    };
                let error = (fitted - exact).abs() / exact.max(1e-4);
                assert!(
                    error < tolerance,
                    "roughness {roughness}, nv {nv}, disc {disc}: LTC {fitted} vs {exact} ({error})"
                );
            }
            // Diffuse: the identity transform gives the clamped cosine exactly.
            let lambert = |l: Vec3| l.z.max(0.) / PI;
            let mut exact = 0.;
            let steps = 400;
            let normal = x.cross(y).normalize();
            let area = 4. * x.cross(y).length() / (steps * steps) as f32;
            for j in 0..steps {
                for i in 0..steps {
                    let (s, t) = (
                        (i as f32 + 0.5) / steps as f32 * 2. - 1.,
                        (j as f32 + 0.5) / steps as f32 * 2. - 1.,
                    );
                    let p = centre + x * s + y * t;
                    let d2 = p.length_squared();
                    let l = p / d2.sqrt();
                    exact += lambert(l) * l.dot(normal).abs() / d2 * area;
                }
            }
            let fitted = polygon(
                Mat3::IDENTITY,
                &[
                    centre - x - y,
                    centre + x - y,
                    centre + x + y,
                    centre - x + y,
                ],
            );
            assert!(
                (fitted - exact).abs() < 0.05 * exact.max(0.01),
                "diffuse {fitted} vs {exact}"
            );
        }
    }
}
