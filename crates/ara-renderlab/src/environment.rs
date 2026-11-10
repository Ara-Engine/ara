//! Ara's equirectangular skies (an authored HDRI, or the gradient sky) prepared for
//! Renderlab's `Sky::Map`: a cosine-convolved irradiance cube and a GGX-prefiltered
//! radiance chain in Renderlab's `PreparedEnvironment` layout. Renderlab's cook does
//! this offline; the bridge does it once per sky, on the CPU, when the sky changes.
//!
//! Ara and Renderlab share the equirectangular convention (+Y up,
//! `u = atan2(z, x) / 2pi + 0.5`, `v = acos(y) / pi`), so no rotation is applied.
use anyhow::{Context, Result};
use renderlab::asset::environment::{
    CubeFace, CubeLevel, Limits, Metadata, PreparedEnvironment, RadianceUnits,
};
use std::f64::consts::{PI, TAU};

/// Face size of the irradiance cube: irradiance has no detail finer than this.
const DIFFUSE_SIZE: u32 = 16;
/// Directions the irradiance convolution integrates over.
const DIFFUSE_SAMPLES: u32 = 2048;
/// GGX directions per prefiltered texel; filtered sampling keeps this few noise-free.
const SPECULAR_SAMPLES: u32 = 64;
/// Largest face of the radiance chain. Its first level is the visible background.
const MAX_FACE: u32 = 512;

/// Texels a content key reads at most: every texel of the gradient sky, an even
/// spread of a large map (the key is computed every frame).
const KEY_TEXELS: usize = 16_384;

/// A content key for an equirectangular image: its size and an even spread of its
/// texels.
pub(crate) fn key(size: [u32; 2], pixels: &[[f32; 4]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ (u64::from(size[0]) << 32 | u64::from(size[1]));
    for pixel in pixels.iter().step_by((pixels.len() / KEY_TEXELS).max(1)) {
        for channel in &pixel[..3] {
            hash = (hash ^ u64::from(channel.to_bits())).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// An equirectangular image with a box-filtered mip pyramid, sampled by direction.
struct Source {
    /// Width, height and texels of each level, largest first.
    levels: Vec<(usize, usize, Vec<[f32; 3]>)>,
}

impl Source {
    fn new(size: [u32; 2], pixels: &[[f32; 4]]) -> Result<Self> {
        let (width, height) = (size[0] as usize, size[1] as usize);
        anyhow::ensure!(
            width > 0 && height > 0 && pixels.len() == width * height,
            "environment map of {width}x{height} has {} texels",
            pixels.len()
        );
        // Radiance is finite and nonnegative for Renderlab; a bad texel is black.
        let clean = |v: f32| if v.is_finite() && v > 0. { v } else { 0. };
        let base: Vec<[f32; 3]> = pixels
            .iter()
            .map(|p| [clean(p[0]), clean(p[1]), clean(p[2])])
            .collect();
        let mut levels = vec![(width, height, base)];
        while let Some((w, h, texels)) = levels.last()
            && (*w > 1 || *h > 1)
        {
            let (nw, nh) = ((*w / 2).max(1), (*h / 2).max(1));
            let mut next = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                for x in 0..nw {
                    let mut sum = [0f32; 3];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let texel = texels[(y * 2 + dy).min(h - 1) * w + (x * 2 + dx).min(w - 1)];
                        for (s, c) in sum.iter_mut().zip(texel) {
                            *s += c * 0.25;
                        }
                    }
                    next.push(sum);
                }
            }
            levels.push((nw, nh, next));
        }
        Ok(Self { levels })
    }

    /// Solid angle of one texel of the largest level, averaged over the sphere.
    fn texel_solid_angle(&self) -> f64 {
        let (width, height, _) = &self.levels[0];
        4. * PI / (*width * *height) as f64
    }

    /// Bilinear radiance along `d` in one level: longitude wraps, latitude clamps.
    fn bilinear(&self, level: usize, d: [f64; 3]) -> [f64; 3] {
        let (width, height, texels) = &self.levels[level];
        let x = (d[2].atan2(d[0]) / TAU + 0.5) * *width as f64 - 0.5;
        let y = d[1].clamp(-1., 1.).acos() / PI * *height as f64 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let texel = |x: i64, y: i64| {
            texels[y.clamp(0, *height as i64 - 1) as usize * width
                + x.rem_euclid(*width as i64) as usize]
        };
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, e) = (
            texel(x0, y0),
            texel(x0 + 1, y0),
            texel(x0, y0 + 1),
            texel(x0 + 1, y0 + 1),
        );
        std::array::from_fn(|i| {
            (f64::from(a[i]) * (1. - tx) + f64::from(b[i]) * tx) * (1. - ty)
                + (f64::from(c[i]) * (1. - tx) + f64::from(e[i]) * tx) * ty
        })
    }

    /// Trilinear radiance along `d` at a fractional pyramid level.
    fn sample(&self, d: [f64; 3], lod: f64) -> [f64; 3] {
        let lod = lod.clamp(0., (self.levels.len() - 1) as f64);
        let low = lod.floor() as usize;
        let t = lod - low as f64;
        let a = self.bilinear(low, d);
        if t <= 0. || low + 1 >= self.levels.len() {
            return a;
        }
        let b = self.bilinear(low + 1, d);
        std::array::from_fn(|i| a[i] * (1. - t) + b[i] * t)
    }
}

/// One cube level filled by `texel(direction)`, in Renderlab's face order and rows.
fn cube(size: u32, mut texel: impl FnMut([f64; 3]) -> [f64; 3]) -> Result<CubeLevel> {
    let mut texels = Vec::with_capacity(6 * (size * size) as usize);
    for face in CubeFace::ALL {
        for y in 0..size {
            for x in 0..size {
                let uv = [x, y].map(|v| 2. * (f64::from(v) + 0.5) / f64::from(size) - 1.);
                texels.push(texel(face.direction(uv)).map(|v| v.max(0.) as f32));
            }
        }
    }
    CubeLevel::new(size, texels, Limits::unbounded()).context("environment cube level")
}

/// An orthonormal tangent and bitangent around the unit vector `n`.
fn basis(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let up = if n[1].abs() < 0.99 {
        [0., 1., 0.]
    } else {
        [1., 0., 0.]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(up, n);
    let length = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    let t = t.map(|v| v / length);
    (t, cross(n, t))
}

/// Irradiance (`E = integral of L cos`, in lux for cd/m² input) toward each direction.
fn irradiance(source: &Source) -> Result<CubeLevel> {
    // Equal-area directions over the sphere, read from a level whose texels are about
    // as large as the spacing between them, so a small bright sun is never missed.
    let spacing = (4. * PI / f64::from(DIFFUSE_SAMPLES)).max(source.texel_solid_angle());
    let lod = 0.5 * (spacing / source.texel_solid_angle()).log2();
    let golden = PI * (3. - 5f64.sqrt());
    let table: Vec<([f64; 3], [f64; 3])> = (0..DIFFUSE_SAMPLES)
        .map(|i| {
            let y = 1. - 2. * (f64::from(i) + 0.5) / f64::from(DIFFUSE_SAMPLES);
            let radius = (1. - y * y).sqrt();
            let phi = f64::from(i) * golden;
            let d = [radius * phi.cos(), y, radius * phi.sin()];
            (d, source.sample(d, lod))
        })
        .collect();
    cube(DIFFUSE_SIZE, |n| {
        let (mut sum, mut weight) = ([0f64; 3], 0f64);
        for (d, radiance) in &table {
            let cosine = (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]).max(0.);
            for (s, r) in sum.iter_mut().zip(radiance) {
                *s += r * cosine;
            }
            weight += cosine;
        }
        sum.map(|s| s / weight * PI)
    })
}

/// The radiance chain: level 0 is the map itself, level `i` of `count` is GGX-filtered
/// at perceptual roughness `i / (count - 1)` with the normal, view and reflection
/// directions equal (Renderlab's convention).
fn radiance(source: &Source, face: u32) -> Result<Vec<CubeLevel>> {
    let count = face.ilog2() + 1;
    let texel_angle = source.texel_solid_angle();
    (0..count)
        .map(|level| {
            let size = face >> level;
            if level == 0 {
                // One cube texel may cover several source texels: read the level that matches.
                let cube_angle = 4. * PI / (6. * f64::from(size * size));
                let lod = 0.5 * (cube_angle / texel_angle).max(1.).log2();
                return cube(size, |n| source.sample(n, lod));
            }
            let roughness = f64::from(level) / f64::from(count - 1);
            let alpha2 = (roughness * roughness).max(1e-4).powi(2);
            // GGX half-vectors around +Z with each reflected direction's density.
            let samples: Vec<([f64; 3], f64)> = (0..SPECULAR_SAMPLES)
                .filter_map(|i| {
                    let phi = TAU * (f64::from(i) + 0.5) / f64::from(SPECULAR_SAMPLES);
                    let u = f64::from(i.reverse_bits()) / 4_294_967_296.;
                    let cos_h = ((1. - u) / (1. + (alpha2 - 1.) * u)).sqrt();
                    let sin_h = (1. - cos_h * cos_h).max(0.).sqrt();
                    // Reflecting the view (+Z) about the half-vector.
                    let l = [
                        2. * cos_h * sin_h * phi.cos(),
                        2. * cos_h * sin_h * phi.sin(),
                        2. * cos_h * cos_h - 1.,
                    ];
                    let d = alpha2 / (PI * (cos_h * cos_h * (alpha2 - 1.) + 1.).powi(2));
                    // Each sample stands for the solid angle 1 / (N pdf): read the level
                    // whose texels are that large (filtered importance sampling).
                    let angle = 1. / (f64::from(SPECULAR_SAMPLES) * (d / 4.).max(1e-9));
                    let lod = 0.5 * (angle / texel_angle).max(1.).log2() + 1.;
                    (l[2] > 0.).then_some((l, lod))
                })
                .collect();
            cube(size, |n| {
                let (t, b) = basis(n);
                let (mut sum, mut weight) = ([0f64; 3], 0f64);
                for (l, lod) in &samples {
                    let world = std::array::from_fn(|i| t[i] * l[0] + b[i] * l[1] + n[i] * l[2]);
                    let value = source.sample(world, *lod);
                    for (s, v) in sum.iter_mut().zip(value) {
                        *s += v * l[2];
                    }
                    weight += l[2];
                }
                sum.map(|s| s / weight)
            })
        })
        .collect()
}

/// Prepares the equirectangular image `pixels` (linear RGB radiance in cd/m², alpha
/// ignored) of `size` for `Engine::create_environment`.
pub(crate) fn prepare(size: [u32; 2], pixels: &[[f32; 4]]) -> Result<PreparedEnvironment> {
    let source = Source::new(size, pixels)?;
    // A cube face spans a quarter of the map's width: no finer than the source, capped.
    let face = (size[0] / 4).max(16).next_power_of_two().min(MAX_FACE);
    let mut generator_hash = [0; 32];
    for (chunk, salt) in generator_hash.as_chunks_mut::<8>().0.iter_mut().zip(0u64..) {
        chunk.copy_from_slice(&key(size, pixels).wrapping_add(salt).to_le_bytes());
    }
    PreparedEnvironment::new(
        Metadata {
            units: RadianceUnits::CandelasPerSquareMetre,
            rotation_y_radians: 0.,
            source_radiance_scale: 1.,
            generator_hash,
        },
        irradiance(&source)?,
        radiance(&source, face)?,
        Limits::unbounded(),
    )
    .context("prepared environment")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn texel(level: &CubeLevel, face: CubeFace, x: u32, y: u32) -> [f32; 3] {
        level.face(face)[(y * level.size() + x) as usize]
    }

    #[test]
    fn a_constant_sky_stays_constant_at_every_roughness_and_gives_pi_times_its_radiance() {
        let pixels = vec![[2., 3., 4., 1.]; 64 * 32];
        let prepared = prepare([64, 32], &pixels).unwrap();
        for level in prepared.specular_radiance() {
            for texel in level.texels() {
                for (value, expected) in texel.iter().zip([2., 3., 4.]) {
                    assert!((value - expected).abs() < 1e-3, "{value} vs {expected}");
                }
            }
        }
        for texel in prepared.diffuse_irradiance().texels() {
            assert!((texel[0] - 2. * std::f32::consts::PI).abs() < 1e-2, "{texel:?}");
        }
    }

    #[test]
    fn a_bright_upper_hemisphere_lights_the_up_face_and_not_the_down_face() {
        // Rows above the horizon are 10 cd/m², rows below are black.
        let pixels: Vec<[f32; 4]> = (0..32)
            .flat_map(|y| std::iter::repeat_n(if y < 16 { [10.; 4] } else { [0.; 4] }, 64))
            .collect();
        let prepared = prepare([64, 32], &pixels).unwrap();
        let sharp = &prepared.specular_radiance()[0];
        let mid = sharp.size() / 2;
        assert!(texel(sharp, CubeFace::PositiveY, mid, mid)[0] > 9.9);
        assert!(texel(sharp, CubeFace::NegativeY, mid, mid)[0] < 0.1);
        // A hemisphere of radiance L gives pi L straight up and nothing straight down.
        let diffuse = prepared.diffuse_irradiance();
        let mid = diffuse.size() / 2;
        let up = texel(diffuse, CubeFace::PositiveY, mid, mid)[0];
        let down = texel(diffuse, CubeFace::NegativeY, mid, mid)[0];
        assert!((up - 10. * std::f32::consts::PI).abs() < 1.5, "{up}");
        assert!(down < 1.5, "{down}");
        // The roughest level blurs the horizon: sideways sees about half the sky.
        let rough = prepared.specular_radiance().last().unwrap();
        let side = texel(rough, CubeFace::PositiveX, 0, 0)[0];
        assert!((2. ..8.).contains(&side), "{side}");
    }

    #[test]
    fn the_key_changes_with_content_and_size() {
        let pixels = vec![[1., 1., 1., 1.]; 8];
        let mut other = pixels.clone();
        other[3][1] = 2.;
        assert_ne!(key([4, 2], &pixels), key([4, 2], &other));
        assert_ne!(key([4, 2], &pixels), key([2, 4], &pixels));
    }
}
