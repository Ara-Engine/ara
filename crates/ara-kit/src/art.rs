//! Generated cover art for things without a picture yet (a new project, a template without a
//! screenshot): soft colour fields from Ara's chart palette, seeded by an id so each keeps its
//! own, with a little grain and a vignette so it reads as a picture, not a flat tile. Drawn on
//! the CPU once per id and size; cache the result.
use stillkit::{Image, Rgba};

use crate::look::Look;

/// A `w` x `h` cover for `seed` in `look`'s palette, as premultiplied BGRA.
pub fn cover(look: &Look, seed: u64, w: u32, h: u32) -> Image {
    let c = &look.c;
    let palette = [c.chart_1.0, c.chart_2.0, c.chart_3.0, c.chart_4.0, c.chart_5.0, c.chart_6.0];
    let mut rng = Rng(seed ^ 0x9e37_79b9_7f4a_7c15);
    // Two or three tinted lights and a deep base from the scheme's background.
    let base = if look.dark { mix(c.background.0, palette[(seed % 6) as usize], 0.18) } else { mix(c.card.0, palette[(seed % 6) as usize], 0.12) };
    let lights: Vec<(f32, f32, f32, Rgba)> = (0..3)
        .map(|i| {
            let colour = palette[((seed / 7 + i * 2) % 6) as usize];
            (rng.f() * 1.3 - 0.15, rng.f() * 1.3 - 0.15, 0.6 + rng.f() * 0.5, colour)
        })
        .collect();
    let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
    let aspect = fw / fh;
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / fw, y as f32 / fh);
            let mut col = [base.r, base.g, base.b];
            for &(lx, ly, radius, tint) in &lights {
                let (dx, dy) = ((u - lx) * aspect, v - ly);
                let d = (dx * dx + dy * dy).sqrt() / radius;
                // A Gaussian glow: wide, with no edge to find.
                let k = (-d * d * 2.0).exp() * if look.dark { 0.6 } else { 0.42 };
                for (ch, t) in col.iter_mut().zip([tint.r, tint.g, tint.b]) {
                    *ch += (t - *ch) * k;
                }
            }
            // A vignette toward the corners and a whisper of grain against banding.
            let (cx, cy) = (u - 0.5, v - 0.5);
            let vignette = 1.0 - (cx * cx + cy * cy) * if look.dark { 0.9 } else { 0.35 };
            let grain = (hash(x, y, seed) - 0.5) * 0.025;
            let px = |ch: f32| ((ch * vignette + grain).clamp(0.0, 1.0) * 255.0).round() as u8;
            pixels.extend_from_slice(&[px(col[2]), px(col[1]), px(col[0]), 255]);
        }
    }
    Image { width: w, height: h, pixels }
}

/// A stable 64-bit seed for an id string (FNV-1a).
pub fn seed(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba::new(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1.0)
}

fn hash(x: u32, y: u32, seed: u64) -> f32 {
    let mut h = u64::from(x).wrapping_mul(0x9e37_79b9) ^ u64::from(y).wrapping_mul(0x85eb_ca6b) ^ seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    (h & 0xffff) as f32 / 65535.0
}

/// A tiny deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_stable_per_seed_and_differ_between_seeds() {
        let look = Look::new(true);
        let a = cover(&look, seed("outpost"), 64, 36);
        assert_eq!(a.pixels, cover(&look, seed("outpost"), 64, 36).pixels);
        assert_ne!(a.pixels, cover(&look, seed("night-market"), 64, 36).pixels);
        assert_eq!(a.pixels.len(), 64 * 36 * 4);
        assert!(a.pixels.chunks(4).all(|p| p[3] == 255), "opaque");
    }
}
