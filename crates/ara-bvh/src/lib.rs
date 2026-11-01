//! CPU construction of compressed-wide BVHs (CWBVH) for Ara's software `trace()`:
//! one BLAS per mesh asset, built at import or on load from a mesh-hash cache, plus
//! the instance TLAS. Pure data in, flat GPU-ready buffers out; no GPU or asset I/O
//! (side-car caches go through the caller's [`sidecar`] bytes).
//! Used by `ara-importers` (import-time builds) and `ara-render` (`look::trace`).
//!
//! The pipeline is a port of tinybvh (MIT, Jacco Bikker; see `THIRD_PARTY_NOTICES.md`):
//! a binned SAH builder with spatial splits (SBVH), the SAH-optimal collapse to an
//! eight-wide tree (Ylitie et al. 2017), and the CWBVH encoding of 80-byte nodes with
//! quantized child boxes plus 64-byte precomputed triangles (Baldwin and Weber 2016),
//! byte-for-byte the layout R4-1/R5-6 measured. Every step is single-threaded and
//! deterministic: the same mesh bytes give the same BLAS bytes on every machine.

mod collapse;
mod cwbvh;
mod mesh;
mod sbvh;
pub mod sidecar;
pub mod tlas;
pub mod traverse;

#[cfg(test)]
mod tests;

pub use cwbvh::{Blas, BlasStats, NODE_WORDS, TRIANGLE_WORDS};
pub use mesh::{BuildError, Mesh, MeshHash};
pub use traverse::{Hit, Ray, RayFlags};

use glam::Vec3;

/// Version of the build algorithms and the encoded layout. Part of every side-car
/// key: bump it whenever a change alters the bytes a mesh builds to.
pub const BUILDER_VERSION: u32 = 2;

/// Traversal stack entries `shaders/trace.slangh` reserves per BLAS (`TRACE_STACK`).
/// Every BLAS this crate builds needs at most this many ([`BlasStats::max_stack`]):
/// the binary build caps its depth and the collapse falls back to a depth-bounded
/// grouping when the SAH-optimal tree would be deeper.
pub const TRACE_STACK: u32 = 12;

/// Axis-aligned bounds. An empty box has `min > max` on every axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Smallest corner.
    pub min: Vec3,
    /// Largest corner.
    pub max: Vec3,
}

impl Aabb {
    /// The box that contains nothing; growing it by a point gives that point.
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::MAX),
        max: Vec3::splat(-f32::MAX),
    };

    /// Box of three points.
    pub fn of_triangle(t: &[Vec3; 3]) -> Self {
        Self {
            min: t[0].min(t[1]).min(t[2]),
            max: t[0].max(t[1]).max(t[2]),
        }
    }
    /// Grows the box to contain `p`.
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    /// Smallest box containing both.
    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
    /// Whether the box contains at least one point.
    pub fn is_valid(&self) -> bool {
        self.min.cmple(self.max).all()
    }
    /// Half the surface area (the SAH weight); zero for an empty box.
    pub fn half_area(&self) -> f32 {
        let e = self.max - self.min;
        if e.x < 0.0 || e.y < 0.0 || e.z < 0.0 {
            return 0.0;
        }
        e.x * e.y + e.y * e.z + e.z * e.x
    }
    /// Centre point.
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    /// Whether `p` lies inside, boundary included.
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
    /// The world box of this object-space box under an affine `transform`
    /// (the eight transformed corners).
    pub fn transformed(&self, transform: &glam::Affine3A) -> Self {
        let mut out = Self::EMPTY;
        for corner in 0..8 {
            let p = Vec3::new(
                if corner & 1 == 0 {
                    self.min.x
                } else {
                    self.max.x
                },
                if corner & 2 == 0 {
                    self.min.y
                } else {
                    self.max.y
                },
                if corner & 4 == 0 {
                    self.min.z
                } else {
                    self.max.z
                },
            );
            out.grow(transform.transform_point3(p));
        }
        out
    }
}
