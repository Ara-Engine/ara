// Ported to Rust from tinybvh (tiny_bvh_base.h at c9be3103cc9693b456b0a0353927c11cac7d710f,
// BVH::BuildHQTask, SplitFrag, ClipFragToBins, SplitSmallExact, SweepBins); Ara changes:
// depth-capped construction with balanced median fallback, leaves of at most three
// references, an intersection-box overlap test and compacted subtree reference ranges.
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

//! Binary BVH construction: binned SAH object splits plus SBVH spatial splits (Stich
//! et al. 2009) that clip straddling triangles, within a reference budget of 1.5x the
//! triangles. Leaves hold at most [`MAX_LEAF`] references and no leaf lies deeper than
//! [`MAX_DEPTH`]: nodes whose SAH split would break that switch to balanced median
//! splits, which keeps the wide tree within the shader's traversal stack.
use crate::Aabb;
use glam::Vec3;

/// Most references per binary leaf (one CWBVH leaf slot holds three triangles).
pub(crate) const MAX_LEAF: u32 = 3;
/// Deepest binary leaf, in edges from the root. Three binary levels per wide level
/// (the depth-bounded collapse) keep the wide tree within [`crate::TRACE_STACK`].
pub(crate) const MAX_DEPTH: u32 = 3 * (crate::TRACE_STACK + 1);
/// Nodes this small sweep their sorted fragments exactly instead of binning.
const EXACT_PRIMS: usize = 8;
const C_TRAV: f32 = 1.0;
const C_INT: f32 = 1.0;

/// Builder knobs; [`Default`] is the configuration Ara ships.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    /// Clip straddling triangles at spatial split planes (SBVH).
    pub spatial_splits: bool,
    /// Bins per axis for object and spatial sweeps (2 to 64).
    pub bins: u32,
    /// Spatial splits are tried only where the object split's children overlap by
    /// more than this share of the root area (or no object split helps).
    pub min_spatial_overlap: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            spatial_splits: true,
            bins: 16,
            min_spatial_overlap: 1e-5,
        }
    }
}

/// A triangle reference: the (possibly clipped) bounds of source triangle `prim`.
#[derive(Debug, Clone, Copy)]
struct Fragment {
    bounds: Aabb,
    prim: u32,
    clipped: bool,
}

/// Binary node: a leaf when `count > 0` (references `first..first + count`),
/// otherwise children `first` and `first + 1`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Node {
    pub bounds: Aabb,
    pub first: u32,
    pub count: u32,
}

impl Node {
    pub(crate) fn is_leaf(&self) -> bool {
        self.count > 0
    }
}

/// A built binary tree. Children always follow their parent; every subtree's leaves
/// reference one contiguous range of `refs` (source triangle indices).
pub(crate) struct Binary {
    pub nodes: Vec<Node>,
    pub refs: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
struct Task {
    node: u32,
    depth: u32,
    start: u32,
    end: u32,
}

/// The chosen split of one node.
enum Split {
    Object {
        axis: usize,
        pos: u32,
        left: u32,
        right: u32,
    },
    Spatial {
        axis: usize,
        pos: u32,
        left: u32,
        right: u32,
    },
    /// Fragments in this order, the first `k` on the left (exact sweeps).
    Ordered { order: Vec<u32>, k: usize },
}

impl Split {
    /// The larger side's reference count, an upper bound before unsplitting.
    fn larger_side(&self) -> u32 {
        match self {
            Self::Object { left, right, .. } | Self::Spatial { left, right, .. } => {
                (*left).max(*right)
            }
            Self::Ordered { order, k } => (*k).max(order.len() - k) as u32,
        }
    }
}

struct Builder<'a> {
    tris: &'a [[Vec3; 3]],
    frags: Vec<Fragment>,
    idx: Vec<u32>,
    tmp: Vec<u32>,
    nodes: Vec<Node>,
    settings: Settings,
    root_area: f32,
    min_dim: Vec3,
}

/// Binary levels a balanced subtree over `count` references needs.
fn levels_needed(count: u32) -> u32 {
    let leaves = count.div_ceil(MAX_LEAF).max(1);
    u32::BITS - (leaves - 1).leading_zeros()
}

fn split_cost(r_area: f32, al: f32, nl: u32, ar: f32, nr: u32) -> f32 {
    C_TRAV + C_INT * r_area * (al * nl as f32 + ar * nr as f32)
}

/// Builds the binary tree over `tris` (non-degenerate triangles; at least one).
pub(crate) fn build(tris: &[[Vec3; 3]], settings: &Settings) -> Binary {
    let n = tris.len();
    let slack = if settings.spatial_splits { n / 2 } else { 0 };
    let cap = n + slack;
    let frags: Vec<Fragment> = tris
        .iter()
        .enumerate()
        .map(|(i, t)| Fragment {
            bounds: Aabb::of_triangle(t),
            prim: i as u32,
            clipped: false,
        })
        .collect();
    let root = frags.iter().fold(Aabb::EMPTY, |b, f| b.union(f.bounds));
    let mut idx: Vec<u32> = (0..n as u32).collect();
    idx.resize(cap, 0);
    let mut b = Builder {
        tris,
        frags,
        idx,
        tmp: vec![0; cap],
        nodes: vec![Node {
            bounds: root,
            first: 0,
            count: n as u32,
        }],
        settings: *settings,
        root_area: root.half_area().max(f32::MIN_POSITIVE),
        min_dim: (root.max - root.min) * 1e-7,
    };
    let mut tasks = vec![Task {
        node: 0,
        depth: 0,
        start: 0,
        end: cap as u32,
    }];
    while let Some(mut task) = tasks.pop() {
        while let Some((left, right)) = b.subdivide(task) {
            tasks.push(right);
            task = left;
        }
    }
    b.finish()
}

impl Builder<'_> {
    fn frag(&self, i: u32) -> &Fragment {
        &self.frags[self.idx[i as usize] as usize]
    }

    /// Splits `task`'s node in two (returning the child tasks) or leaves it a leaf.
    fn subdivide(&mut self, task: Task) -> Option<(Task, Task)> {
        let node = self.nodes[task.node as usize];
        let count = node.count;
        if count <= 1 || (count <= MAX_LEAF && task.depth >= MAX_DEPTH) {
            return None;
        }
        // An SAH leaf is allowed only when it fits a CWBVH leaf slot; otherwise the
        // node splits, by SAH while its children stay within the depth cap and by
        // centroid median (which halves the depth still needed) once they would not.
        let must = count > MAX_LEAF;
        let fits = |side: u32| task.depth + 1 + levels_needed(side) <= MAX_DEPTH;
        let split = if !fits(count - 1) {
            None
        } else if count as usize <= EXACT_PRIMS {
            self.exact(&node, must)
        } else {
            self.binned(&node, task, must)
        };
        let (a, b) = match split.filter(|s| fits(s.larger_side())) {
            Some(split) => self.partition(&node, task, split),
            None if !must => return None,
            None => self.median(&node, task),
        };
        // A spatial partition that left one side empty split no fragment, so the
        // node's references are intact for the fallback.
        let (a, b) = if a == task.start || b == task.end {
            if !must {
                return None;
            }
            self.median(&node, task)
        } else {
            (a, b)
        };
        self.idx[task.start as usize..task.end as usize]
            .copy_from_slice(&self.tmp[task.start as usize..task.end as usize]);
        let bounds = |s: &Self, from: u32, to: u32| {
            (from..to).fold(Aabb::EMPTY, |acc, i| acc.union(s.frag(i).bounds))
        };
        let left = self.nodes.len() as u32;
        let (lb, rb) = (bounds(self, task.start, a), bounds(self, b, task.end));
        self.nodes.push(Node {
            bounds: lb,
            first: task.start,
            count: a - task.start,
        });
        self.nodes.push(Node {
            bounds: rb,
            first: b,
            count: task.end - b,
        });
        self.nodes[task.node as usize] = Node {
            bounds: node.bounds,
            first: left,
            count: 0,
        };
        let mid = (a + b) / 2;
        Some((
            Task {
                node: left,
                depth: task.depth + 1,
                start: task.start,
                end: mid,
            },
            Task {
                node: left + 1,
                depth: task.depth + 1,
                start: mid,
                end: task.end,
            },
        ))
    }

    /// Exact SAH object split over the sorted centroids of a small node.
    fn exact(&self, node: &Node, must: bool) -> Option<Split> {
        let n = node.count as usize;
        let r_area = 1.0 / node.bounds.half_area().max(f32::MIN_POSITIVE);
        let mut best = if must {
            f32::INFINITY
        } else {
            n as f32 * C_INT
        };
        let mut found = None;
        let extent = node.bounds.max - node.bounds.min;
        for axis in 0..3 {
            if extent[axis] <= self.min_dim[axis] {
                continue;
            }
            let order = self.sorted(node, axis);
            let mut right = vec![Aabb::EMPTY; n];
            let mut acc = Aabb::EMPTY;
            for k in (1..n).rev() {
                acc = acc.union(self.frags[order[k] as usize].bounds);
                right[k] = acc;
            }
            let mut left = Aabb::EMPTY;
            for k in 1..n {
                left = left.union(self.frags[order[k - 1] as usize].bounds);
                let c = split_cost(
                    r_area,
                    left.half_area(),
                    k as u32,
                    right[k].half_area(),
                    (n - k) as u32,
                );
                if c < best {
                    best = c;
                    found = Some((order.clone(), k));
                }
            }
        }
        found.map(|(order, k)| Split::Ordered { order, k })
    }

    /// The node's fragments sorted by centroid along `axis` (ties by fragment).
    fn sorted(&self, node: &Node, axis: usize) -> Vec<u32> {
        let mut order: Vec<u32> = (node.first..node.first + node.count)
            .map(|i| self.idx[i as usize])
            .collect();
        order.sort_by(|&a, &b| {
            let ca = self.frags[a as usize].bounds.center()[axis];
            let cb = self.frags[b as usize].bounds.center()[axis];
            ca.total_cmp(&cb).then(a.cmp(&b))
        });
        order
    }

    /// Best binned object split, then the best spatial split if it beats it.
    fn binned(&self, node: &Node, task: Task, must: bool) -> Option<Split> {
        let bins = self.settings.bins as usize;
        let bounds = node.bounds;
        let extent = bounds.max - bounds.min;
        let rpd = Vec3::splat(bins as f32) / extent;
        let mut boxes = vec![Aabb::EMPTY; 3 * bins];
        let mut counts = vec![0u32; 3 * bins];
        for i in node.first..node.first + node.count {
            let f = self.frag(i);
            let c = (f.bounds.center() - bounds.min) * rpd;
            for axis in 0..3 {
                let bin = axis * bins + (c[axis] as i32).clamp(0, bins as i32 - 1) as usize;
                boxes[bin] = boxes[bin].union(f.bounds);
                counts[bin] += 1;
            }
        }
        let no_split = node.count as f32 * C_INT;
        let r_area = 1.0 / bounds.half_area().max(f32::MIN_POSITIVE);
        let mut best_cost = if must { f32::INFINITY } else { no_split };
        let mut best: Option<(usize, u32, Aabb, Aabb, u32, u32)> = None;
        for axis in 0..3 {
            if extent[axis] <= self.min_dim[axis] {
                continue;
            }
            let range = axis * bins..(axis + 1) * bins;
            sweep(
                &boxes[range.clone()],
                &counts[range.clone()],
                &counts[range],
                r_area,
                |i, l, nl, r, nr, c| {
                    if c < best_cost {
                        best_cost = c;
                        best = Some((axis, i as u32, l, r, nl, nr));
                    }
                },
            );
        }
        let mut split = best.map(|(axis, pos, _, _, left, right)| Split::Object {
            axis,
            pos,
            left,
            right,
        });
        let budget = task.end - task.start;
        let overlap = best.map_or(0.0, |(_, _, l, r, _, _)| {
            let i = Aabb {
                min: l.min.max(r.min),
                max: l.max.min(r.max),
            };
            i.half_area() / self.root_area
        });
        if self.settings.spatial_splits
            && budget > node.count
            && (overlap > self.settings.min_spatial_overlap || best_cost >= no_split)
        {
            let mut min_cost = best_cost * 0.985;
            let mut axes = [false; 3];
            for (axis, valid) in axes.iter_mut().enumerate() {
                *valid = extent[axis] > self.min_dim[axis];
            }
            let smallest = if extent.x < extent.y {
                if extent.x < extent.z { 0 } else { 2 }
            } else if extent.y < extent.z {
                1
            } else {
                2
            };
            if axes.iter().enumerate().any(|(a, &v)| v && a != smallest) {
                axes[smallest] = false;
            }
            for (axis, _) in axes.iter().enumerate().filter(|(_, v)| **v) {
                let plane = extent[axis] / (bins as f32 * 0.9999);
                let (sboxes, cin, cout) = self.bin_spatial(node, axis, plane);
                sweep(&sboxes, &cin, &cout, r_area, |i, _, nl, _, nr, c| {
                    if c < min_cost && nl + nr < budget && nl > 0 && nr > 0 {
                        min_cost = c;
                        split = Some(Split::Spatial {
                            axis,
                            pos: i as u32,
                            left: nl,
                            right: nr,
                        });
                    }
                });
            }
        }
        split
    }

    /// Spatial binning along `axis`: each fragment clipped into the bins it spans.
    fn bin_spatial(&self, node: &Node, axis: usize, plane: f32) -> (Vec<Aabb>, Vec<u32>, Vec<u32>) {
        let bins = self.settings.bins as usize;
        let mut boxes = vec![Aabb::EMPTY; bins];
        let (mut cin, mut cout) = (vec![0u32; bins], vec![0u32; bins]);
        let lo = node.bounds.min[axis];
        let r_plane = 1.0 / plane;
        let bin = |v: f32| (((v - lo) * r_plane) as i32).clamp(0, bins as i32 - 1) as usize;
        for i in node.first..node.first + node.count {
            let f = self.frag(i);
            let (b1, b2) = (bin(f.bounds.min[axis]), bin(f.bounds.max[axis]));
            cin[b1] += 1;
            cout[b2] += 1;
            if b1 == b2 {
                boxes[b1] = boxes[b1].union(f.bounds);
            } else {
                self.clip_to_bins(f, b1, b2, axis, lo, plane, &mut boxes);
            }
        }
        (boxes, cin, cout)
    }

    /// Adds the part of `f`'s triangle inside each bin `b1..=b2` to that bin.
    #[allow(clippy::too_many_arguments)]
    fn clip_to_bins(
        &self,
        f: &Fragment,
        b1: usize,
        b2: usize,
        axis: usize,
        lo: f32,
        plane: f32,
        boxes: &mut [Aabb],
    ) {
        let mut poly = [Vec3::ZERO; 9];
        poly[..3].copy_from_slice(&self.tris[f.prim as usize]);
        let mut n = 3;
        let (mut left, mut right) = ([Vec3::ZERO; 9], [Vec3::ZERO; 9]);
        let mut j = b1 as i64 - 1;
        while j <= b2 as i64 && n > 0 {
            let pos = lo + plane * (j + 1) as f32;
            let (mut nl, mut nr) = (0, 0);
            for k in 0..n {
                let a = poly[k];
                let b = poly[if k + 1 == n { 0 } else { k + 1 }];
                let (ina, inb) = (a[axis] <= pos, b[axis] <= pos);
                if ina {
                    left[nl] = a;
                    nl += 1;
                } else {
                    right[nr] = a;
                    nr += 1;
                }
                if ina != inb {
                    let mut c = a + (pos - a[axis]) / (b[axis] - a[axis]) * (b - a);
                    c[axis] = pos;
                    left[nl] = c;
                    right[nr] = c;
                    nl += 1;
                    nr += 1;
                }
            }
            if j >= b1 as i64 && nl > 0 {
                let mut part = Aabb::EMPTY;
                for p in &left[..nl] {
                    part.grow(*p);
                }
                part = widen(part, axis, Aabb::of_triangle(&self.tris[f.prim as usize]));
                if f.clipped {
                    part.min = part.min.max(f.bounds.min);
                    part.max = part.max.min(f.bounds.max);
                }
                if part.half_area() > 0.0 {
                    let jb = j as usize;
                    boxes[jb] = boxes[jb].union(part);
                }
            }
            if j == b2 as i64 {
                break;
            }
            poly[..nr].copy_from_slice(&right[..nr]);
            n = nr;
            j += 1;
        }
    }

    /// Writes the node's references into `tmp` for `split` (left from the slice
    /// start, right from its end) and returns the two boundaries.
    fn partition(&mut self, node: &Node, task: Task, split: Split) -> (u32, u32) {
        let (mut a, mut b) = (task.start, task.end);
        let bounds = node.bounds;
        match split {
            Split::Ordered { order, k } => {
                for (i, &f) in order.iter().enumerate() {
                    if i < k {
                        self.tmp[a as usize] = f;
                        a += 1;
                    } else {
                        b -= 1;
                        self.tmp[b as usize] = f;
                    }
                }
            }
            Split::Object { axis, pos, .. } => {
                let bins = self.settings.bins as i32;
                let rpd = bins as f32 / (bounds.max[axis] - bounds.min[axis]);
                for i in node.first..node.first + node.count {
                    let f = self.idx[i as usize];
                    let c = self.frags[f as usize].bounds.center()[axis];
                    let bin = (((c - bounds.min[axis]) * rpd) as i32).clamp(0, bins - 1);
                    if bin <= pos as i32 {
                        self.tmp[a as usize] = f;
                        a += 1;
                    } else {
                        b -= 1;
                        self.tmp[b as usize] = f;
                    }
                }
            }
            Split::Spatial {
                axis,
                pos,
                left,
                right,
            } => {
                let r_area = 1.0 / bounds.half_area().max(f32::MIN_POSITIVE);
                let plane =
                    (bounds.max[axis] - bounds.min[axis]) / (self.settings.bins as f32 * 0.9999);
                let lo = bounds.min[axis];
                let (r_plane, last) = (1.0 / plane, self.settings.bins as i32 - 1);
                let at = |v: f32| (((v - lo) * r_plane) as i32).clamp(0, last) as u32;
                // Bounds of the sides so far, for the unsplitting tests.
                let (mut nl, mut nr) = (left, right);
                let split_pos = lo + plane * (pos + 1) as f32;
                let (mut lbox, mut rbox) = self.spatial_sides(node, axis, at, pos);
                let mut cost = split_cost(r_area, lbox.half_area(), nl, rbox.half_area(), nr);
                for i in node.first..node.first + node.count {
                    let fi = self.idx[i as usize];
                    let f = self.frags[fi as usize];
                    let (b1, b2) = (at(f.bounds.min[axis]), at(f.bounds.max[axis]));
                    if b2 <= pos {
                        self.tmp[a as usize] = fi;
                        a += 1;
                        continue;
                    }
                    if b1 > pos {
                        b -= 1;
                        self.tmp[b as usize] = fi;
                        continue;
                    }
                    // A straddler: keep it whole on one side if that is cheaper.
                    if nr > 1 {
                        let l2 = lbox.union(f.bounds);
                        let c = split_cost(r_area, l2.half_area(), nl, rbox.half_area(), nr - 1);
                        if c <= cost {
                            nr -= 1;
                            cost = c;
                            lbox = l2;
                            self.tmp[a as usize] = fi;
                            a += 1;
                            continue;
                        }
                    }
                    if nl > 1 {
                        let r2 = rbox.union(f.bounds);
                        let c = split_cost(r_area, lbox.half_area(), nl - 1, r2.half_area(), nr);
                        if c <= cost {
                            nl -= 1;
                            cost = c;
                            rbox = r2;
                            b -= 1;
                            self.tmp[b as usize] = fi;
                            continue;
                        }
                    }
                    let (l, r, valid) = self.split_fragment(&f, axis, split_pos);
                    if valid == 3 {
                        let new = self.frags.len() as u32;
                        self.frags[fi as usize] = l;
                        self.frags.push(r);
                        self.tmp[a as usize] = fi;
                        a += 1;
                        b -= 1;
                        self.tmp[b as usize] = new;
                    } else if valid & 1 != 0 {
                        self.tmp[a as usize] = fi;
                        a += 1;
                    } else {
                        b -= 1;
                        self.tmp[b as usize] = fi;
                    }
                }
            }
        }
        (a, b)
    }

    /// The two side boxes of a spatial split at bin `pos` before straddlers are
    /// placed: the clipped parts of every fragment on each side of the plane.
    fn spatial_sides(
        &self,
        node: &Node,
        axis: usize,
        at: impl Fn(f32) -> u32,
        pos: u32,
    ) -> (Aabb, Aabb) {
        let bounds = node.bounds;
        let plane = (bounds.max[axis] - bounds.min[axis]) / (self.settings.bins as f32 * 0.9999);
        let split_pos = bounds.min[axis] + plane * (pos + 1) as f32;
        let (mut l, mut r) = (Aabb::EMPTY, Aabb::EMPTY);
        for i in node.first..node.first + node.count {
            let f = self.frag(i);
            let (b1, b2) = (at(f.bounds.min[axis]), at(f.bounds.max[axis]));
            if b2 <= pos {
                l = l.union(f.bounds);
            } else if b1 > pos {
                r = r.union(f.bounds);
            } else {
                let (fl, fr, valid) = self.split_fragment(f, axis, split_pos);
                if valid & 1 != 0 {
                    l = l.union(fl.bounds);
                }
                if valid & 2 != 0 {
                    r = r.union(fr.bounds);
                }
            }
        }
        (l, r)
    }

    /// Cuts fragment `f` at `pos` along `axis`; bit 0/1 of the mask: left/right valid.
    fn split_fragment(&self, f: &Fragment, axis: usize, pos: f32) -> (Fragment, Fragment, u32) {
        let mut l = Fragment {
            bounds: Aabb::EMPTY,
            prim: f.prim,
            clipped: true,
        };
        let mut r = l;
        let v = self.tris[f.prim as usize];
        let edge = |a: Vec3, b: Vec3| {
            let mut c = a + (pos - a[axis]) / (b[axis] - a[axis]) * (b - a);
            c[axis] = pos;
            c
        };
        let side = v.map(|p| p[axis] <= pos);
        for k in 0..3 {
            if side[k] {
                l.bounds.grow(v[k]);
            } else {
                r.bounds.grow(v[k]);
            }
            let k2 = (k + 1) % 3;
            if side[k] != side[k2] {
                let c = edge(v[k], v[k2]);
                l.bounds.grow(c);
                r.bounds.grow(c);
            }
        }
        let whole = Aabb::of_triangle(&v);
        for part in [&mut l, &mut r] {
            part.bounds = widen(part.bounds, axis, whole);
            if f.clipped {
                part.bounds.min = part.bounds.min.max(f.bounds.min);
                part.bounds.max = part.bounds.max.min(f.bounds.max);
            }
        }
        let ok = |b: &Aabb| b.is_valid() && b.half_area() > 0.0;
        let mask = u32::from(ok(&l.bounds)) | (u32::from(ok(&r.bounds)) << 1);
        (l, r, mask)
    }

    /// Balanced split: the node's fragments by centroid along the widest centroid
    /// axis, half on each side.
    fn median(&mut self, node: &Node, task: Task) -> (u32, u32) {
        let mut centroids = Aabb::EMPTY;
        for i in node.first..node.first + node.count {
            centroids.grow(self.frag(i).bounds.center());
        }
        let e = centroids.max - centroids.min;
        let axis = if e.x >= e.y && e.x >= e.z {
            0
        } else if e.y >= e.z {
            1
        } else {
            2
        };
        let order = self.sorted(node, axis);
        let k = order.len().div_ceil(2);
        self.partition(node, task, Split::Ordered { order, k })
    }

    /// Maps leaf fragments to source triangles and gives every subtree one
    /// contiguous reference range (leaves in depth-first, left-first order).
    fn finish(mut self) -> Binary {
        let mut refs = Vec::with_capacity(self.frags.len());
        let mut stack = vec![0u32];
        while let Some(n) = stack.pop() {
            let node = self.nodes[n as usize];
            if node.is_leaf() {
                let first = refs.len() as u32;
                for i in node.first..node.first + node.count {
                    refs.push(self.frags[self.idx[i as usize] as usize].prim);
                }
                self.nodes[n as usize].first = first;
            } else {
                stack.push(node.first + 1);
                stack.push(node.first);
            }
        }
        Binary {
            nodes: self.nodes,
            refs,
        }
    }
}

/// Grows a clipped part's box by a few ulps on the axes other than the split `axis`
/// (clip points there are rounded; on the split axis they are exact), never past the
/// whole triangle's box, so the part's box always contains the true clipped polygon.
fn widen(part: Aabb, axis: usize, whole: Aabb) -> Aabb {
    let mut out = part;
    for a in (0..3).filter(|&a| a != axis) {
        let eps = whole.min[a].abs().max(whole.max[a].abs()) * (1.0 / (1 << 20) as f32);
        out.min[a] = (part.min[a] - eps).max(whole.min[a]);
        out.max[a] = (part.max[a] + eps).min(whole.max[a]);
    }
    out
}

/// SAH sweep over one axis' bins: `accept(plane, left box, left count, right box,
/// right count, cost)` for every plane between bins with both sides non-empty.
fn sweep(
    boxes: &[Aabb],
    cl: &[u32],
    cr: &[u32],
    r_area: f32,
    mut accept: impl FnMut(usize, Aabb, u32, Aabb, u32, f32),
) {
    let bins = boxes.len();
    let mut rb = vec![Aabb::EMPTY; bins];
    let mut rn = vec![0u32; bins];
    let (mut r, mut n) = (Aabb::EMPTY, 0);
    for j in (0..bins - 1).rev() {
        r = r.union(boxes[j + 1]);
        n += cr[j + 1];
        rb[j] = r;
        rn[j] = n;
    }
    let (mut l, mut nl) = (Aabb::EMPTY, 0);
    for i in 0..bins - 1 {
        l = l.union(boxes[i]);
        nl += cl[i];
        if nl == 0 || rn[i] == 0 {
            continue;
        }
        let c = split_cost(r_area, l.half_area(), nl, rb[i].half_area(), rn[i]);
        accept(i, l, nl, rb[i], rn[i], c);
    }
}
