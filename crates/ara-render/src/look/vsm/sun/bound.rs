//! The conservative invalidation bound of a sheared sun page (council-v3 3.1) and the
//! height index it reads.
//!
//! A page drawn under shear `k0` and read under `k1` misplaces a shadow edge cast by
//! caster point `c` onto receiver point `r` by exactly `(c.y - r.y) |k1 - k0|` in ground
//! coordinates (both rays pass through `c`; [`super::oblique`]), for any receiver
//! orientation. A page whose casters reach at most `top` and whose receivers lie in
//! `[lo, hi]` therefore lags by at most `(top - lo) |dk|`. Filter taps that cross into
//! a neighbouring page read texels drawn under that page's shear; the shader moves each
//! into the receiver page's frame by `(y - y_ref)(k_neighbour - k_page)`
//! (shadow-pages.slangh `spTexel`), so they are exact up to the neighbour's own lag,
//! which the neighbour's bound covers (receivers near an edge mark the neighbour too).
//!
//! [`Heights`] bounds `top` and `[lo, hi]` from instance world boxes (every surface
//! lies inside its instance's box), so it is conservative without GPU feedback; GPU
//! receiver feedback (`VSM.Mark`) only narrows `[lo, hi]` to the receivers that
//! actually sample the page.
use super::oblique::sheared_rect;
use glam::{IVec2, Vec2, Vec3};
use std::collections::HashMap;

/// Ground cell of the height index, metres.
const CELL: f32 = 4.;
/// Instances spanning more cells than this per axis are listed once, globally.
const SPAN: i32 = 16;

/// Heights that bound one page's shadow motion: the highest caster point and the
/// receivers' height range (world metres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageHeights {
    pub top: f32,
    pub receivers: [f32; 2],
}

impl PageHeights {
    /// Upper bound, in metres of ground coordinates, on any shadow edge's misplacement
    /// in this page per unit of shear change: the caster height above the lowest receiver.
    pub(crate) fn reach(&self) -> f32 {
        (self.top - self.receivers[0]).max(0.)
    }

    /// The bound in page texels (`texel` metres) after a shear change of `dk`.
    pub(crate) fn lag_texels(&self, dk: f32, texel: f32) -> f32 {
        self.reach() * dk / texel
    }
}

/// Instance world boxes by ground cell: per page, the highest box top and the box
/// height range among instances whose sheared footprint meets the page (`page`).
/// Moved instances join their new cells (stale entries stay; candidates are tested
/// against current boxes) until stale entries outnumber instances, then it rebuilds.
#[derive(Default)]
pub(crate) struct Heights {
    bounds: Vec<[Vec3; 2]>,
    cells: HashMap<IVec2, Vec<u32>>,
    big: Vec<u32>,
    stale: usize,
    /// Every instance's height range.
    y: [f32; 2],
}

impl Heights {
    /// Rebuilds the index over `bounds` (world boxes, one per instance row).
    pub(crate) fn rebuild(&mut self, bounds: &[[Vec3; 2]]) {
        self.bounds.clear();
        self.bounds.extend_from_slice(bounds);
        self.cells.clear();
        self.big.clear();
        self.stale = 0;
        self.y = [f32::INFINITY, f32::NEG_INFINITY];
        for i in 0..self.bounds.len() {
            self.insert(i);
        }
    }

    /// Follows the current boxes `bounds` (same rows as the last rebuild): changed
    /// rows join their new cells; a different row count rebuilds.
    pub(crate) fn update(&mut self, bounds: &[[Vec3; 2]]) {
        if bounds.len() != self.bounds.len() {
            self.rebuild(bounds);
            return;
        }
        for (i, b) in bounds.iter().enumerate() {
            if self.bounds[i] != *b {
                self.bounds[i] = *b;
                self.stale += 1;
                self.insert(i);
            }
        }
        if self.stale > self.bounds.len().max(64) {
            let all = std::mem::take(&mut self.bounds);
            self.rebuild(&all);
        }
    }

    /// The height range of every instance box (empty: `[inf, -inf]`).
    pub(crate) fn span(&self) -> [f32; 2] {
        self.y
    }

    fn insert(&mut self, i: usize) {
        let [lo, hi] = self.bounds[i];
        self.y = [self.y[0].min(lo.y), self.y[1].max(hi.y)];
        let a = (Vec2::new(lo.x, lo.z) / CELL).floor().as_ivec2();
        let b = (Vec2::new(hi.x, hi.z) / CELL).floor().as_ivec2();
        if (b - a).max_element() >= SPAN {
            self.big.push(i as u32);
            return;
        }
        for y in a.y..=b.y {
            for x in a.x..=b.x {
                self.cells
                    .entry(IVec2::new(x, y))
                    .or_default()
                    .push(i as u32);
            }
        }
    }

    /// The heights of ground rectangle `lo..hi` under shear `k` (reference `y_ref`):
    /// over instances whose sheared box meets it; `None` where none does (nothing
    /// casts or receives there). `scratch` holds candidates between calls.
    pub(crate) fn page(
        &self,
        (lo, hi): (Vec2, Vec2),
        k: Vec2,
        y_ref: f32,
        scratch: &mut Vec<u32>,
    ) -> Option<PageHeights> {
        if self.bounds.is_empty() {
            return None;
        }
        // Instances that can shear into the rectangle lie in it swept along +k over
        // the content's height range.
        let (s0, s1) = ((self.y[0] - y_ref) * k, (self.y[1] - y_ref) * k);
        let (rlo, rhi) = (lo + s0.min(s1), hi + s0.max(s1));
        let a = (rlo / CELL).floor().as_ivec2();
        let b = (rhi / CELL).floor().as_ivec2();
        scratch.clear();
        for y in a.y..=b.y {
            for x in a.x..=b.x {
                if let Some(cell) = self.cells.get(&IVec2::new(x, y)) {
                    scratch.extend_from_slice(cell);
                }
            }
        }
        scratch.extend_from_slice(&self.big);
        scratch.sort_unstable();
        scratch.dedup();
        let mut found: Option<PageHeights> = None;
        for &i in scratch.iter() {
            let b = self.bounds[i as usize];
            let (blo, bhi) = sheared_rect(b, k, y_ref);
            if blo.cmpgt(hi).any() || bhi.cmplt(lo).any() {
                continue;
            }
            let h = found.get_or_insert(PageHeights {
                top: f32::NEG_INFINITY,
                receivers: [f32::INFINITY, f32::NEG_INFINITY],
            });
            h.top = h.top.max(b[1].y);
            h.receivers = [h.receivers[0].min(b[0].y), h.receivers[1].max(b[1].y)];
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::super::oblique::{ground, shear};
    use super::*;

    /// A deterministic generator for the property tests (no RNG enters the renderer).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    fn sun(elevation: f32, azimuth: f32) -> Vec3 {
        Vec3::new(
            elevation.cos() * azimuth.cos(),
            elevation.sin(),
            elevation.cos() * azimuth.sin(),
        )
    }

    /// The shadow of caster point `c` along `d` on plane (`point`, `normal`).
    fn shadow_on(c: Vec3, d: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
        let denom = d.dot(normal);
        if denom.abs() < 1e-4 {
            return None;
        }
        let t = (c - point).dot(normal) / denom;
        (t > 0.).then(|| c - d * t)
    }

    /// Every caster/receiver pair under any sun motion (elevation down to 3 degrees,
    /// azimuth and elevation changes, tilted receivers) moves its shadow edge in the
    /// page's ground coordinates by no more than the bound.
    #[test]
    fn the_bound_holds_for_tilted_receivers_and_low_suns() {
        let mut r = Lcg(7);
        let y_ref = 0.25;
        let mut worst: f32 = 0.;
        for _ in 0..20_000 {
            let e0 = r.range(3f32.to_radians(), 80f32.to_radians());
            let a0 = r.range(0., std::f32::consts::TAU);
            let (de, da) = (r.range(-0.02, 0.02), r.range(-0.05, 0.05));
            let (d0, d1) = (sun(e0, a0), sun((e0 + de).max(0.06), a0 + da));
            let (Some(k0), Some(k1)) = (shear(d0), shear(d1)) else {
                continue;
            };
            let normal =
                Vec3::new(r.range(-1., 1.), r.range(0.2, 1.), r.range(-1., 1.)).normalize();
            let plane = Vec3::new(r.range(-5., 5.), r.range(-1., 2.), r.range(-5., 5.));
            let c = plane + Vec3::new(r.range(-3., 3.), r.range(0.05, 6.), r.range(-3., 3.));
            if (c - plane).dot(normal) <= 0. {
                continue;
            }
            let (Some(e0p), Some(e1p)) = (
                shadow_on(c, d0, plane, normal),
                shadow_on(c, d1, plane, normal),
            ) else {
                continue;
            };
            // Both shadow points read through the page drawn under k0.
            let moved = (ground(e1p, k0, y_ref) - ground(e0p, k0, y_ref)).length();
            let lo = e0p.y.min(e1p.y);
            let heights = PageHeights {
                top: c.y,
                receivers: [lo, e0p.y.max(e1p.y)],
            };
            let bound = heights.reach() * (k1 - k0).length();
            assert!(
                moved <= bound * (1. + 1e-3) + 1e-6,
                "moved {moved} > bound {bound}: d0 {d0} d1 {d1} c {c} n {normal}"
            );
            // The caster-receiver term alone is exact for the receiver point: equality
            // up to f32 rounding (shadows land up to ~100 m away at 3 degrees), so the
            // bound is tight, not loose by construction.
            let exact = (c.y - e1p.y) * (k1 - k0).length();
            assert!(
                (moved - exact).abs() <= 2e-3 * exact + 2e-4,
                "moved {moved} exact {exact}"
            );
            worst = worst.max(moved / bound.max(1e-9));
        }
        assert!(
            worst > 0.3,
            "the property sweep exercised large ratios: {worst}"
        );
    }

    #[test]
    fn heights_cover_instances_that_shear_into_a_page() {
        let k = shear(sun(15f32.to_radians(), 0.4)).expect("elevated");
        let mut heights = Heights::default();
        let pole = [Vec3::new(0., 0., 0.), Vec3::new(0.1, 3., 0.1)];
        let floor = [Vec3::new(-50., -0.2, -50.), Vec3::new(50., 0., 50.)];
        heights.rebuild(&[floor, pole]);
        let mut scratch = Vec::new();
        // The pole's top shears to -3 k from its base: a page there sees it.
        let top_u = ground(Vec3::new(0.05, 3., 0.05), k, 0.);
        let h = heights
            .page((top_u - 0.01, top_u + 0.01), k, 0., &mut scratch)
            .expect("the floor and the pole meet it");
        assert_eq!(h.top, 3.);
        assert_eq!(h.receivers, [-0.2, 3.]);
        // Far from the pole only the floor slab.
        let far = heights
            .page(
                (Vec2::new(30., 30.), Vec2::new(31., 31.)),
                k,
                0.,
                &mut scratch,
            )
            .expect("floor");
        assert_eq!(far.top, 0.);
        assert!(far.reach() <= 0.21, "{far:?}");
        // A moved pole follows.
        let moved = [Vec3::new(20., 0., 20.), Vec3::new(20.1, 3., 20.1)];
        heights.update(&[floor, moved]);
        let h = heights
            .page((top_u - 0.01, top_u + 0.01), k, 0., &mut scratch)
            .expect("floor");
        assert_eq!(h.top, 0.);
    }
}
