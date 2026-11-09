//! Sheared ("ground-plane") sun pages: every page is a fixed square of the world's
//! ground grid, and its depth texels hold the height of the first surface along the
//! sun's rays through that square, so a page keeps its place while the sun moves.
//!
//! A world point `p` maps to ground coordinates `u = p.xz - (p.y - y_ref) k` with the
//! shear `k = d.xz / d.y` of the unit direction `d` towards the sun: `u` is constant
//! along every sun ray, so a shadow test is "is the stored top height at `u(p)` above
//! `p.y`". When the sun moves from `k0` to `k1`, the shadow of a caster point `c` on a
//! receiver point `r` moves by exactly `(c.y - r.y) |k1 - k0|` in `u` (any receiver
//! orientation; [`super::bound`]), which is the council-v3 3.1 bound
//! `h * delta-alpha / sin^2(alpha)` plus the azimuth term, without a small-angle step.
use glam::{IVec2, Mat4, Vec2, Vec3, Vec4};

/// Lowest sun elevation (radians) the sheared pages serve (sin 2 degrees): below it
/// `|k|` exceeds ~29 and a page's casters smear over kilometres, so the service
/// reports no pages and shading keeps the cascades.
pub(crate) const MIN_SIN_ELEVATION: f32 = 0.034_899_5;

/// The shear `k = d.xz / d.y` of `sun` (unit, towards the light); `None` below
/// [`MIN_SIN_ELEVATION`].
pub(crate) fn shear(sun: Vec3) -> Option<Vec2> {
    let d = sun.normalize_or_zero();
    (d.y >= MIN_SIN_ELEVATION).then(|| Vec2::new(d.x, d.z) / d.y)
}

/// Ground coordinates of world `p` under shear `k` about reference height `y_ref` (the
/// shaders' `spGround`; the CPU needs only the rectangles of whole boxes).
#[cfg(test)]
pub(crate) fn ground(p: Vec3, k: Vec2, y_ref: f32) -> Vec2 {
    Vec2::new(p.x, p.z) - (p.y - y_ref) * k
}

/// The sheared light view of world page `page` at `level` (page width `step` metres):
/// its ground square to the whole clip square (x along ground x, y along ground z,
/// rows from the square's +z edge as the cooked `-fvk-invert-y` raster writes them),
/// depth the world height over `depth = [y_lo, y_hi]` (reversed-Z: higher is nearer the
/// sun, so the GREATER test keeps the first surface along each sun ray).
pub(crate) fn page_matrix(k: Vec2, y_ref: f32, depth: [f32; 2], step: f32, page: IVec2) -> Mat4 {
    let lo = page.as_vec2() * step;
    let s = 2. / step;
    let a = 1. / (depth[1] - depth[0]);
    let b = -depth[0] * a;
    // clip.x = s (x - (y - y_ref) k.x - lo.x) - 1, clip.y likewise with z and k.y.
    Mat4::from_cols(
        Vec4::new(s, 0., 0., 0.),
        Vec4::new(-s * k.x, -s * k.y, a, 0.),
        Vec4::new(0., s, 0., 0.),
        Vec4::new(
            s * (k.x * y_ref - lo.x) - 1.,
            s * (k.y * y_ref - lo.y) - 1.,
            b,
            1.,
        ),
    )
}

/// The ground-space bounding box of world box `[lo, hi]` under shear `k`: its xz
/// rectangle swept along `-k` over its height range.
pub(crate) fn sheared_rect([lo, hi]: [Vec3; 2], k: Vec2, y_ref: f32) -> (Vec2, Vec2) {
    let a = (lo.y - y_ref) * k;
    let b = (hi.y - y_ref) * k;
    (
        Vec2::new(lo.x, lo.z) - a.max(b),
        Vec2::new(hi.x, hi.z) - a.min(b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sun(elevation_deg: f32, azimuth_deg: f32) -> Vec3 {
        let (e, a) = (elevation_deg.to_radians(), azimuth_deg.to_radians());
        Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin())
    }

    #[test]
    fn ground_coordinates_are_constant_along_sun_rays() {
        let d = sun(23., 71.);
        let k = shear(d).expect("above the minimum elevation");
        let p = Vec3::new(3.5, 1.25, -7.);
        for t in [0.5_f32, 4., 40.] {
            let q = p + d * t;
            assert!((ground(q, k, 0.3) - ground(p, k, 0.3)).length() < 1e-4);
        }
        assert!(shear(sun(1., 0.)).is_none());
    }

    #[test]
    fn a_page_matrix_maps_its_square_and_heights() {
        let k = shear(sun(30., 200.)).expect("elevated");
        let (step, page, y_ref) = (0.25, IVec2::new(-3, 17), 0.5);
        let m = page_matrix(k, y_ref, [-64., 64.], step, page);
        // Points of the page's ground square at any height land inside the clip square.
        for (fx, fz, y) in [(0.1, 0.2, 0.), (0.9, 0.5, 3.), (0.5, 0.95, -2.)] {
            let u = (page.as_vec2() + Vec2::new(fx, fz)) * step;
            let xz = u + (y - y_ref) * k;
            let clip = m.project_point3(Vec3::new(xz.x, y, xz.y));
            assert!((clip.x - (2. * fx - 1.)).abs() < 1e-4, "{clip}");
            assert!((clip.y - (2. * fz - 1.)).abs() < 1e-4, "{clip}");
            assert!((clip.z - (y + 64.) / 128.).abs() < 1e-5, "{clip}");
        }
    }

    #[test]
    fn sheared_rects_bound_every_corner() {
        let k = shear(sun(12., 33.)).expect("elevated");
        let b = [Vec3::new(-1., 0.5, 2.), Vec3::new(0.5, 3., 2.5)];
        let (lo, hi) = sheared_rect(b, k, 0.);
        for i in 0..8u32 {
            let p = Vec3::new(
                if i & 1 == 0 { b[0].x } else { b[1].x },
                if i & 2 == 0 { b[0].y } else { b[1].y },
                if i & 4 == 0 { b[0].z } else { b[1].z },
            );
            let u = ground(p, k, 0.);
            assert!(u.cmpge(lo - 1e-5).all() && u.cmple(hi + 1e-5).all());
        }
    }
}
