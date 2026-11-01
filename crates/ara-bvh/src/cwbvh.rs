// Ported to Rust from tinybvh (tiny_bvh_base.h at c9be3103cc9693b456b0a0353927c11cac7d710f,
// BVH8_CWBVH::ConvertFrom with CWBVH_COMPRESSED_TRIS, and BVHBase::PrecomputeTriangle);
// Ara changes: conservative quantization in exact arithmetic, a depth-bounded
// collapse, and submesh and orientation words in each triangle's fourth block.
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

//! The CWBVH encoding ("Efficient Incoherent Ray Traversal on GPUs Through Compressed
//! Wide BVHs", Ylitie et al. 2017) as `shaders/trace.slangh` reads it.
//!
//! A node is five 16-byte words:
//! - `n0`: the node's box minimum (f32 xyz) and, in `w`, three 127-biased
//!   power-of-two quantization exponents (bytes 0-2) and the interior-child mask
//!   (byte 3, one bit per slot);
//! - `n1`: `x` the first interior child (node index within the BLAS), `y` the first
//!   triangle (16-byte word within the BLAS), `z`/`w` one meta byte per slot (interior:
//!   `0b001_11000 + slot`; leaf: unary triangle count in bits 5-7, the triangle offset
//!   in bits 0-4; zero: empty);
//! - `n2..n4`: per slot, quantized child bounds: lo x, lo y, lo z, hi x, hi y, hi z.
//!
//! A triangle is four 16-byte words: three rows of the world-to-barycentric transform
//! (Baldwin and Weber 2016; `u` and `v` weigh corners 1 and 2, the third row is the
//! plane) and `[submesh, flags, 0, primitive]`, flag bit 0 meaning the plane row's
//! dominant normal axis is negative (front faces then have a positive plane dot).
use crate::collapse::{self, Child, Wide};
use crate::mesh::{BuildError, Mesh, MeshHash};
use crate::sbvh::{self, Settings};
use crate::{Aabb, TRACE_STACK};
use glam::Vec3;

/// 16-byte words per CWBVH node.
pub const NODE_WORDS: usize = 5;
/// 16-byte words per compressed triangle.
pub const TRIANGLE_WORDS: usize = 4;
/// Triangle flag: the plane row's dominant normal component is negative.
pub const FLIPPED: u32 = 1;

/// Build statistics of one BLAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlasStats {
    /// Input triangles.
    pub triangles: u32,
    /// Input triangles left out because they can never be hit (zero area or
    /// non-finite corners).
    pub degenerate: u32,
    /// Triangle references after spatial splits (each one 64 bytes).
    pub references: u32,
    /// CWBVH nodes (each one 80 bytes).
    pub nodes: u32,
    /// Traversal stack entries the deepest path needs (at most [`TRACE_STACK`]).
    pub max_stack: u32,
    /// Submeshes.
    pub submeshes: u32,
}

/// One mesh's bottom-level CWBVH, ready to upload: node and triangle words relative
/// to the BLAS (the TLAS instance record adds the BLAS's base offsets).
#[derive(Debug, Clone, PartialEq)]
pub struct Blas {
    /// The mesh content hash this BLAS was built from.
    pub hash: MeshHash,
    /// [`NODE_WORDS`] words per node; node 0 is the root.
    pub nodes: Vec<[u32; 4]>,
    /// [`TRIANGLE_WORDS`] words per triangle reference.
    pub triangles: Vec<[u32; 4]>,
    /// Object-space bounds of every hittable triangle (empty when there is none).
    pub bounds: Aabb,
    /// Build statistics.
    pub stats: BlasStats,
}

impl Blas {
    /// Builds the BLAS of `mesh` with Ara's shipped settings (binned SAH with
    /// spatial splits, then the SAH-optimal eight-wide collapse).
    pub fn build(mesh: &Mesh<'_>) -> Result<Self, BuildError> {
        Self::build_with(mesh, &Settings::default())
    }

    pub(crate) fn build_with(mesh: &Mesh<'_>, settings: &Settings) -> Result<Self, BuildError> {
        mesh.validate()?;
        let total = mesh.triangle_count() as u32;
        let mut tris = Vec::with_capacity(total as usize);
        let mut source = Vec::with_capacity(total as usize);
        for t in 0..total {
            let c = mesh.corners(t);
            if hittable(&c) {
                tris.push(c);
                source.push(t);
            }
        }
        let mut stats = BlasStats {
            triangles: total,
            degenerate: total - tris.len() as u32,
            submeshes: mesh.submeshes.len().max(1) as u32,
            ..BlasStats::default()
        };
        if tris.is_empty() {
            // An empty root: no child slot is ever hit.
            stats.nodes = 1;
            return Ok(Self {
                hash: mesh.hash(),
                nodes: vec![[0; 4]; NODE_WORDS],
                triangles: Vec::new(),
                bounds: Aabb::EMPTY,
                stats,
            });
        }
        let mut binary = sbvh::build(&tris, settings);
        for r in &mut binary.refs {
            *r = source[*r as usize];
        }
        let wide = collapse::collapse(&binary);
        let depth = wide.depth();
        debug_assert!(depth <= TRACE_STACK + 1);
        let (nodes, triangles) = encode(&wide, &binary.refs, mesh);
        stats.references = binary.refs.len() as u32;
        stats.nodes = (nodes.len() / NODE_WORDS) as u32;
        stats.max_stack = depth - 1;
        Ok(Self {
            hash: mesh.hash(),
            nodes,
            triangles,
            bounds: binary.nodes[0].bounds,
            stats,
        })
    }

    /// GPU bytes of this BLAS (nodes plus triangles).
    pub fn bytes(&self) -> usize {
        (self.nodes.len() + self.triangles.len()) * 16
    }
}

/// Whether a triangle can ever be hit: finite corners and a non-zero normal.
fn hittable(c: &[Vec3; 3]) -> bool {
    c.iter().all(|p| p.is_finite()) && (c[1] - c[0]).cross(c[2] - c[0]) != Vec3::ZERO
}

/// Quantization exponent for a node extent: the smallest `e` with `255 * 2^e >=
/// extent`, in exact arithmetic.
fn quant_exp(extent: f64) -> i32 {
    if extent.is_nan() || extent <= 0.0 {
        return -126;
    }
    let mut e = (extent / 255.0).log2().ceil().clamp(-126.0, 126.0) as i32;
    while e < 126 && 255.0 * (e as f64).exp2() < extent {
        e += 1;
    }
    while e > -126 && 255.0 * ((e - 1) as f64).exp2() >= extent {
        e -= 1;
    }
    e
}

/// Encodes the wide tree depth-first; children of one node are written next to
/// each other in slot order.
fn encode(wide: &Wide, refs: &[u32], mesh: &Mesh<'_>) -> (Vec<[u32; 4]>, Vec<[u32; 4]>) {
    let mut nodes = vec![[0u32; 4]; NODE_WORDS];
    let mut tris: Vec<[u32; 4]> = Vec::with_capacity(refs.len() * TRIANGLE_WORDS);
    let mut stack = vec![(0u32, 0usize)];
    while let Some((src, addr)) = stack.pop() {
        let node = &wide.nodes[src as usize];
        let (lo, hi) = (node.bounds.min, node.bounds.max);
        let slots = assign_slots(node, wide);
        let e: [i32; 3] = std::array::from_fn(|a| quant_exp(f64::from(hi[a]) - f64::from(lo[a])));
        let mut bytes = [0u8; 80];
        bytes[0..4].copy_from_slice(&lo.x.to_le_bytes());
        bytes[4..8].copy_from_slice(&lo.y.to_le_bytes());
        bytes[8..12].copy_from_slice(&lo.z.to_le_bytes());
        for a in 0..3 {
            bytes[12 + a] = (e[a] + 127) as u8;
        }
        let (mut imask, mut first_child, mut first_tri) = (0u8, 0u32, 0u32);
        let (mut interior, mut leaf_tris) = (0, 0u32);
        for (slot, child) in slots.iter().enumerate() {
            let Some(child) = child else { continue };
            let bounds = match child {
                Child::Node(c) => wide.nodes[*c as usize].bounds,
                Child::Leaf { bounds, .. } => *bounds,
            };
            for a in 0..3 {
                let step = (e[a] as f64).exp2();
                let qlo = ((f64::from(bounds.min[a]) - f64::from(lo[a])) / step).floor();
                let qhi = ((f64::from(bounds.max[a]) - f64::from(lo[a])) / step).ceil();
                bytes[32 + a * 8 + slot] = qlo.clamp(0.0, 255.0) as u8;
                bytes[56 + a * 8 + slot] = qhi.clamp(0.0, 255.0) as u8;
            }
            match child {
                Child::Node(c) => {
                    let child_addr = nodes.len();
                    nodes.resize(child_addr + NODE_WORDS, [0; 4]);
                    if interior == 0 {
                        first_child = (child_addr / NODE_WORDS) as u32;
                    }
                    interior += 1;
                    imask |= 1 << slot;
                    bytes[24 + slot] = (1 << 5) | (24 + slot as u8);
                    stack.push((*c, child_addr));
                }
                Child::Leaf { first, count, .. } => {
                    if leaf_tris == 0 {
                        first_tri = tris.len() as u32;
                    }
                    let unary = [0, 0b001, 0b011, 0b111][*count as usize];
                    bytes[24 + slot] = (unary << 5) | leaf_tris as u8;
                    leaf_tris += count;
                    for &t in &refs[*first as usize..(*first + *count) as usize] {
                        let (rows, flags) = precompute(&mesh.corners(t));
                        tris.push(rows[0]);
                        tris.push(rows[1]);
                        tris.push(rows[2]);
                        tris.push([mesh.submesh_of(t), flags, 0, t]);
                    }
                }
            }
        }
        bytes[15] = imask;
        bytes[16..20].copy_from_slice(&first_child.to_le_bytes());
        bytes[20..24].copy_from_slice(&first_tri.to_le_bytes());
        for w in 0..NODE_WORDS {
            nodes[addr + w] = std::array::from_fn(|k| {
                let o = w * 16 + k * 4;
                u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
            });
        }
    }
    (nodes, tris)
}

/// Greedy octant slot assignment: slot `s` prefers children whose centroid lies
/// furthest along the octant direction `s` (bit 2: -x, bit 1: -y, bit 0: -z), so a
/// ray's octant visits children roughly front to back.
fn assign_slots(node: &collapse::WideNode, wide: &Wide) -> [Option<Child>; 8] {
    let centre = node.bounds.center();
    let n = node.children.len();
    let mut cost = [[f32::MAX; 8]; 8];
    for (i, child) in node.children.iter().enumerate() {
        let bounds = match child {
            Child::Node(c) => wide.nodes[*c as usize].bounds,
            Child::Leaf { bounds, .. } => *bounds,
        };
        let d = bounds.center() - centre;
        for (s, row) in cost.iter_mut().enumerate() {
            let sx = if s & 4 != 0 { -d.x } else { d.x };
            let sy = if s & 2 != 0 { -d.y } else { d.y };
            let sz = if s & 1 != 0 { -d.z } else { d.z };
            row[i] = sx + sy + sz;
        }
    }
    let mut slot_of = [usize::MAX; 8];
    let mut used = [false; 8];
    loop {
        let mut best = (f32::MAX, usize::MAX, usize::MAX);
        for s in 0..8 {
            if used[s] {
                continue;
            }
            for i in 0..n {
                if slot_of[i] == usize::MAX && cost[s][i] < best.0 {
                    best = (cost[s][i], s, i);
                }
            }
        }
        if best.1 == usize::MAX {
            break;
        }
        used[best.1] = true;
        slot_of[best.2] = best.1;
    }
    for slot in slot_of.iter_mut().take(n) {
        if *slot == usize::MAX
            && let Some(s) = used.iter().position(|u| !u)
        {
            used[s] = true;
            *slot = s;
        }
    }
    let mut slots = [None; 8];
    for (i, child) in node.children.iter().enumerate() {
        slots[slot_of[i]] = Some(*child);
    }
    slots
}

/// The Baldwin-Weber transform rows of a triangle (as f32 bits) and its flags.
pub(crate) fn precompute(v: &[Vec3; 3]) -> ([[u32; 4]; 3], u32) {
    let (v0, v1, v2) = (v[0], v[1], v[2]);
    let (e1, e2) = (v1 - v0, v2 - v0);
    let n = e1.cross(e2);
    let d = v0.dot(n);
    let mut t = [0f32; 12];
    let flipped;
    if n.x.abs() > n.y.abs() && n.x.abs() > n.z.abs() {
        let (x1, x2) = (v1.y * v0.z - v1.z * v0.y, v2.y * v0.z - v2.z * v0.y);
        let r = 1.0 / n.x;
        t = [
            0.0,
            e2.z * r,
            -e2.y * r,
            x2 * r,
            0.0,
            -e1.z * r,
            e1.y * r,
            -x1 * r,
            1.0,
            n.y * r,
            n.z * r,
            -d * r,
        ];
        flipped = n.x < 0.0;
    } else if n.y.abs() > n.z.abs() {
        let (x1, x2) = (v1.z * v0.x - v1.x * v0.z, v2.z * v0.x - v2.x * v0.z);
        let r = 1.0 / n.y;
        t = [
            -e2.z * r,
            0.0,
            e2.x * r,
            x2 * r,
            e1.z * r,
            0.0,
            -e1.x * r,
            -x1 * r,
            n.x * r,
            1.0,
            n.z * r,
            -d * r,
        ];
        flipped = n.y < 0.0;
    } else if n.z.abs() > 0.0 {
        let (x1, x2) = (v1.x * v0.y - v1.y * v0.x, v2.x * v0.y - v2.y * v0.x);
        let r = 1.0 / n.z;
        t = [
            e2.y * r,
            -e2.x * r,
            0.0,
            x2 * r,
            -e1.y * r,
            e1.x * r,
            0.0,
            -x1 * r,
            n.x * r,
            n.y * r,
            1.0,
            -d * r,
        ];
        flipped = n.z < 0.0;
    } else {
        flipped = false;
    }
    let rows = std::array::from_fn(|r| std::array::from_fn(|k| t[r * 4 + k].to_bits()));
    (rows, if flipped { FLIPPED } else { 0 })
}
