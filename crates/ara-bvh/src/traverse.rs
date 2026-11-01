// The CWBVH traversal is ported to Rust from tinybvh (tiny_bvh_base.h at
// c9be3103cc9693b456b0a0353927c11cac7d710f, BVH8_CWBVH::Intersect with
// CWBVH_COMPRESSED_TRIS) and the R4-1 tuned GPU kernel derived from it; Ara adds the
// t-range, ray flags, face orientation and a candidate filter (alpha tests).
//
// The MIT License (MIT)
//
// Copyright (c) 2024-2026, Jacco Bikker / Breda University of Applied Sciences.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

//! CPU ray queries: a BLAS traversal that mirrors `shaders/trace.slangh` operation
//! for operation (tests check the encoding with it; CPU tools may query BLASes the
//! same way), and an f64 brute-force reference over a [`Mesh`].
use crate::cwbvh::{Blas, FLIPPED, NODE_WORDS};
use crate::{Mesh, TRACE_STACK};
use glam::{DVec3, Vec3};

/// A ray: `origin + t * direction` for `t` in `(t_min, t_max)`. The direction need
/// not be unit length; `t` is in its units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Start point.
    pub origin: Vec3,
    /// Direction (any non-zero length).
    pub direction: Vec3,
    /// Hits at or before this are ignored.
    pub t_min: f32,
    /// Hits at or beyond this are ignored.
    pub t_max: f32,
}

/// Query flags, the same bits as `shaders/trace.slangh`'s `TRACE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RayFlags(pub u32);

impl RayFlags {
    /// Closest hit, alpha tests on, both faces.
    pub const NONE: Self = Self(0);
    /// End at the first accepted hit (shadow and visibility queries).
    pub const ACCEPT_FIRST_HIT: Self = Self(1);
    /// Ignore triangles seen from behind.
    pub const CULL_BACK_FACES: Self = Self(2);
    /// Ignore triangles seen from the front.
    pub const CULL_FRONT_FACES: Self = Self(4);
    /// Treat alpha-tested materials as opaque.
    pub const FORCE_OPAQUE: Self = Self(8);

    /// Whether every bit of `other` is set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for RayFlags {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A committed triangle hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Distance along the ray, in direction units.
    pub t: f32,
    /// Source triangle index in the mesh (the stable primitive id).
    pub primitive: u32,
    /// Submesh of that triangle.
    pub submesh: u32,
    /// Weights of corners 1 and 2.
    pub barycentrics: [f32; 2],
    /// Whether the ray met the counter-clockwise (front) side.
    pub front: bool,
}

fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}

impl Blas {
    /// Closest (or, with [`RayFlags::ACCEPT_FIRST_HIT`], any) hit along `ray`.
    pub fn intersect(&self, ray: &Ray, flags: RayFlags) -> Option<Hit> {
        self.intersect_filtered(ray, flags, |_| true)
    }

    /// Like [`Self::intersect`], committing a candidate only when `accept` returns
    /// true (an alpha test).
    pub fn intersect_filtered(
        &self,
        ray: &Ray,
        flags: RayFlags,
        mut accept: impl FnMut(&Hit) -> bool,
    ) -> Option<Hit> {
        let (o, d) = (ray.origin, ray.direction);
        let rd = Vec3::select(d.cmpeq(Vec3::ZERO), Vec3::splat(1e30), d.recip());
        let octinv =
            7 - ((u32::from(d.x < 0.0) << 2) | (u32::from(d.y < 0.0) << 1) | u32::from(d.z < 0.0));
        let mut stack = [[0u32; 2]; TRACE_STACK as usize];
        let mut sp = 0usize;
        let mut ngroup = [0u32, 0x8000_0000];
        let mut tgroup: [u32; 2];
        let (t_min, mut t_max) = (ray.t_min, ray.t_max);
        let mut best = None;
        loop {
            if ngroup[1] > 0x00ff_ffff {
                let imask = ngroup[1];
                let bit = 31 - ngroup[1].leading_zeros();
                let base = ngroup[0];
                ngroup[1] &= !(1 << bit);
                if ngroup[1] > 0x00ff_ffff {
                    assert!(sp < stack.len(), "BLAS deeper than TRACE_STACK");
                    stack[sp] = ngroup;
                    sp += 1;
                }
                let slot = (bit - 24) ^ octinv;
                let rel = (imask & !(u32::MAX << slot)).count_ones();
                let ix = ((base + rel) as usize) * NODE_WORDS;
                let n = &self.nodes[ix..ix + NODE_WORDS];
                let p = Vec3::new(f(n[0][0]), f(n[0][1]), f(n[0][2]));
                let e = n[0][3];
                let scale = Vec3::new(
                    f((e & 255) << 23),
                    f(((e >> 8) & 255) << 23),
                    f(((e >> 16) & 255) << 23),
                ) * rd;
                let orig = (p - o) * rd;
                ngroup = [n[1][0], 0];
                tgroup = [n[1][1], 0];
                let mut mask = 0u32;
                for half in 0..2 {
                    let meta = n[1][2 + half];
                    let inner = (meta & (meta << 1)) & 0x1010_1010;
                    let inner_mask = (inner << 4).wrapping_sub(inner >> 4);
                    let bits =
                        (meta ^ (octinv.wrapping_mul(0x0101_0101) & inner_mask)) & 0x1f1f_1f1f;
                    let children = (meta >> 5) & 0x0707_0707;
                    let pick = |w: usize, neg: bool| -> (u32, u32) {
                        // (lo, hi) quantized words of this half for axis word pair w.
                        let (lo, hi) = match w {
                            0 => (n[2][half], n[3][2 + half]),
                            1 => (n[2][2 + half], n[4][half]),
                            _ => (n[3][half], n[4][2 + half]),
                        };
                        if neg { (hi, lo) } else { (lo, hi) }
                    };
                    let (lx, hx) = pick(0, rd.x < 0.0);
                    let (ly, hy) = pick(1, rd.y < 0.0);
                    let (lz, hz) = pick(2, rd.z < 0.0);
                    for k in 0..4 {
                        let s = k * 8;
                        let q = |w: u32| ((w >> s) & 255) as f32;
                        let lo = Vec3::new(q(lx), q(ly), q(lz)) * scale + orig;
                        let hi = Vec3::new(q(hx), q(hy), q(hz)) * scale + orig;
                        let near = lo.x.max(lo.y).max(lo.z).max(t_min);
                        let far = hi.x.min(hi.y).min(hi.z).min(t_max);
                        if near <= far {
                            mask |= ((children >> s) & 255) << ((bits >> s) & 31);
                        }
                    }
                }
                ngroup[1] = (mask & 0xff00_0000) | (e >> 24);
                tgroup[1] = mask & 0x00ff_ffff;
            } else {
                tgroup = ngroup;
                ngroup = [0, 0];
            }
            while tgroup[1] != 0 {
                let bit = 31 - tgroup[1].leading_zeros();
                let addr = (tgroup[0] + bit * 4) as usize;
                tgroup[1] &= !(1 << bit);
                let row = |r: usize| {
                    let w = self.triangles[addr + r];
                    (Vec3::new(f(w[0]), f(w[1]), f(w[2])), f(w[3]))
                };
                let (n2, w2) = row(2);
                let trans_d = n2.dot(d);
                if trans_d == 0.0 {
                    continue;
                }
                let t = -(n2.dot(o) + w2) / trans_d;
                if !(t > t_min && t < t_max) {
                    continue;
                }
                let i = o + t * d;
                let ((n0, w0), (n1, w1)) = (row(0), row(1));
                let u = n0.dot(i) + w0;
                let v = n1.dot(i) + w1;
                if !(u >= 0.0 && v >= 0.0 && u + v <= 1.0) {
                    continue;
                }
                let meta = self.triangles[addr + 3];
                let front = (trans_d < 0.0) != (meta[1] & FLIPPED != 0);
                if (front && flags.contains(RayFlags::CULL_FRONT_FACES))
                    || (!front && flags.contains(RayFlags::CULL_BACK_FACES))
                {
                    continue;
                }
                let hit = Hit {
                    t,
                    primitive: meta[3],
                    submesh: meta[0],
                    barycentrics: [u, v],
                    front,
                };
                if !accept(&hit) {
                    continue;
                }
                t_max = t;
                best = Some(hit);
                if flags.contains(RayFlags::ACCEPT_FIRST_HIT) {
                    return best;
                }
            }
            if ngroup[1] > 0x00ff_ffff {
                continue;
            }
            if sp == 0 {
                break;
            }
            sp -= 1;
            ngroup = stack[sp];
        }
        best
    }
}

/// Reference closest hit over every triangle of `mesh` (f64 Moller-Trumbore), with
/// the same t-range, face flags and `accept` filter as [`Blas::intersect_filtered`].
/// [`RayFlags::ACCEPT_FIRST_HIT`] is ignored: the reference always finds the closest.
pub fn brute_force(
    mesh: &Mesh<'_>,
    ray: &Ray,
    flags: RayFlags,
    mut accept: impl FnMut(&Hit) -> bool,
) -> Option<Hit> {
    let o = ray.origin.as_dvec3();
    let d = ray.direction.as_dvec3();
    let mut best: Option<Hit> = None;
    let mut t_max = f64::from(ray.t_max);
    for t in 0..mesh.triangle_count() as u32 {
        let c = mesh.corners(t).map(|p| p.as_dvec3());
        let Some((dist, u, v, front)) = moller_trumbore(o, d, &c) else {
            continue;
        };
        if dist <= f64::from(ray.t_min) || dist >= t_max {
            continue;
        }
        if (front && flags.contains(RayFlags::CULL_FRONT_FACES))
            || (!front && flags.contains(RayFlags::CULL_BACK_FACES))
        {
            continue;
        }
        let hit = Hit {
            t: dist as f32,
            primitive: t,
            submesh: mesh.submesh_of(t),
            barycentrics: [u as f32, v as f32],
            front,
        };
        if accept(&hit) {
            t_max = dist;
            best = Some(hit);
        }
    }
    best
}

/// f64 ray-triangle test: distance, weights of corners 1 and 2, and front facing.
pub fn moller_trumbore(o: DVec3, d: DVec3, c: &[DVec3; 3]) -> Option<(f64, f64, f64, bool)> {
    let (e1, e2) = (c[1] - c[0], c[2] - c[0]);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - c[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some((e2.dot(q) * inv, u, v, det > 0.0))
}
