//! The top level: a binary BVH over instance world boxes, stored threaded (stackless)
//! as R4-1 measured it. Each node holds its box and two words: `first` is the next
//! node when the box is hit (the left child, or [`LEAF`] plus the instance for a
//! leaf), `skip` the next node when it is missed or done ([`END`] after the last).
//! Nodes are in depth-first order with the larger child first, so big instances (the
//! world) are tested early and shorten every later test.
//!
//! [`Tlas::refit`] updates boxes for moved instances without changing the tree;
//! [`Tlas::needs_rebuild`] reports when refits have degraded it past
//! [`REBUILD_FACTOR`] times its built SAH cost. Structural changes rebuild.
use crate::Aabb;
use bytemuck::{Pod, Zeroable};

/// `first` flag of a leaf node; the low bits are the instance.
pub const LEAF: u32 = 0x8000_0000;
/// `skip` of the last node in traversal order.
pub const END: u32 = u32::MAX;
/// Refits may grow the SAH cost to this multiple of the built cost before
/// [`Tlas::needs_rebuild`] asks for a rebuild.
pub const REBUILD_FACTOR: f32 = 1.5;
const BINS: usize = 16;

/// One threaded TLAS node (32 bytes, `shaders/trace.slangh` `TraceTlasNode`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TlasNode {
    /// Box minimum.
    pub min: [f32; 3],
    /// Next node on a hit: the left child, or `LEAF | instance`.
    pub first: u32,
    /// Box maximum.
    pub max: [f32; 3],
    /// Next node on a miss (or after a leaf): the subtree's successor, or `END`.
    pub skip: u32,
}

/// A threaded TLAS over a list of instance boxes.
#[derive(Debug, Clone, Default)]
pub struct Tlas {
    /// Nodes in traversal order; empty when no instance has a valid box.
    pub nodes: Vec<TlasNode>,
    /// Node of each input instance, or `u32::MAX` for one left out (empty box).
    leaf: Vec<u32>,
    built_cost: f32,
}

struct Temp {
    bounds: Aabb,
    /// `Some(instance)` for a leaf.
    instance: Option<u32>,
    children: [u32; 2],
}

impl Tlas {
    /// Builds over `boxes` (one per instance; invalid boxes are left out).
    pub fn build(boxes: &[Aabb]) -> Self {
        let mut items: Vec<u32> = (0..boxes.len() as u32)
            .filter(|&i| boxes[i as usize].is_valid())
            .collect();
        let mut tlas = Self {
            nodes: Vec::new(),
            leaf: vec![u32::MAX; boxes.len()],
            built_cost: 0.0,
        };
        if items.is_empty() {
            return tlas;
        }
        let mut temp = Vec::with_capacity(items.len() * 2);
        let len = items.len();
        let root = subdivide(boxes, &mut items, 0, len, &mut temp);
        // Depth-first emission, larger-area child first; skips are linked after.
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            let node = &temp[n as usize];
            let index = tlas.nodes.len() as u32;
            let first = match node.instance {
                Some(i) => {
                    tlas.leaf[i as usize] = index;
                    LEAF | i
                }
                None => index + 1,
            };
            tlas.nodes.push(TlasNode {
                min: node.bounds.min.into(),
                first,
                max: node.bounds.max.into(),
                skip: END,
            });
            if node.instance.is_none() {
                let [a, b] = node.children;
                let b_bigger =
                    temp[b as usize].bounds.half_area() > temp[a as usize].bounds.half_area();
                let (big, small) = if b_bigger { (b, a) } else { (a, b) };
                stack.push(small);
                stack.push(big);
            }
        }
        tlas.link_skips();
        tlas.built_cost = tlas.cost();
        tlas
    }

    /// Links skips: an interior node's first child skips to its second child, which
    /// skips where the parent does (the root ends the walk).
    fn link_skips(&mut self) {
        // The second child starts after the first child's subtree; sizes bottom-up.
        let n = self.nodes.len();
        let mut size = vec![1u32; n];
        for i in (0..n).rev() {
            if self.nodes[i].first & LEAF == 0 {
                let left = i + 1;
                let right = left + size[left] as usize;
                size[i] = 1 + size[left] + size[right];
            }
        }
        for i in 0..n {
            if self.nodes[i].first & LEAF == 0 {
                let left = i + 1;
                let right = (left + size[left] as usize) as u32;
                self.nodes[left].skip = right;
                self.nodes[right as usize].skip = self.nodes[i].skip;
            }
        }
    }

    /// Updates every instance's box in place (`boxes` indexed as at build) and the
    /// interior boxes above them. Returns false, changing nothing, if an instance
    /// gained or lost a valid box: that is a structural change, so rebuild.
    pub fn refit(&mut self, boxes: &[Aabb]) -> bool {
        if boxes.len() != self.leaf.len()
            || boxes
                .iter()
                .zip(&self.leaf)
                .any(|(b, &l)| b.is_valid() != (l != u32::MAX))
        {
            return false;
        }
        for (b, &l) in boxes.iter().zip(&self.leaf) {
            if l != u32::MAX {
                let node = &mut self.nodes[l as usize];
                node.min = b.min.into();
                node.max = b.max.into();
            }
        }
        for i in (0..self.nodes.len()).rev() {
            if self.nodes[i].first & LEAF != 0 {
                continue;
            }
            let left = self.nodes[i + 1];
            let right = self.nodes[left.skip as usize];
            let node = &mut self.nodes[i];
            node.min = std::array::from_fn(|a| left.min[a].min(right.min[a]));
            node.max = std::array::from_fn(|a| left.max[a].max(right.max[a]));
        }
        true
    }

    /// SAH cost of the current tree: the summed node areas over the root's.
    pub fn cost(&self) -> f32 {
        let area = |n: &TlasNode| {
            Aabb {
                min: n.min.into(),
                max: n.max.into(),
            }
            .half_area()
        };
        let Some(root) = self.nodes.first() else {
            return 0.0;
        };
        let root = area(root).max(f32::MIN_POSITIVE);
        self.nodes.iter().map(area).sum::<f32>() / root
    }

    /// Whether refits degraded the tree past [`REBUILD_FACTOR`] times its built cost.
    pub fn needs_rebuild(&self) -> bool {
        self.cost() > self.built_cost * REBUILD_FACTOR
    }

    /// Node holding instance `i`, if it was included.
    pub fn leaf_node(&self, i: u32) -> Option<u32> {
        self.leaf
            .get(i as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
    }
}

/// Builds the subtree over `items[from..to]` into `temp`, returning its index.
fn subdivide(
    boxes: &[Aabb],
    items: &mut [u32],
    from: usize,
    to: usize,
    temp: &mut Vec<Temp>,
) -> u32 {
    // Explicit work list: (from, to, slot in temp to fill).
    let root = temp.len() as u32;
    temp.push(Temp {
        bounds: Aabb::EMPTY,
        instance: None,
        children: [0; 2],
    });
    let mut work = vec![(from, to, root)];
    while let Some((from, to, slot)) = work.pop() {
        let bounds = items[from..to]
            .iter()
            .fold(Aabb::EMPTY, |b, &i| b.union(boxes[i as usize]));
        temp[slot as usize].bounds = bounds;
        if to - from == 1 {
            temp[slot as usize].instance = Some(items[from]);
            continue;
        }
        let mid = split(boxes, &mut items[from..to]) + from;
        let (l, r) = (temp.len() as u32, temp.len() as u32 + 1);
        for _ in 0..2 {
            temp.push(Temp {
                bounds: Aabb::EMPTY,
                instance: None,
                children: [0; 2],
            });
        }
        temp[slot as usize].children = [l, r];
        work.push((mid, to, r));
        work.push((from, mid, l));
    }
    root
}

/// Partitions `items` (at least two) by the best binned SAH centroid split, or the
/// index median when centroids coincide; returns the left count.
fn split(boxes: &[Aabb], items: &mut [u32]) -> usize {
    let mut centroids = Aabb::EMPTY;
    for &i in items.iter() {
        centroids.grow(boxes[i as usize].center());
    }
    let extent = centroids.max - centroids.min;
    let mut best = (f32::INFINITY, 0usize, 0usize);
    for axis in 0..3 {
        if extent[axis] <= 0.0 {
            continue;
        }
        let scale = BINS as f32 / extent[axis];
        let bin = |i: u32| {
            (((boxes[i as usize].center()[axis] - centroids.min[axis]) * scale) as usize)
                .min(BINS - 1)
        };
        let mut bin_box = [Aabb::EMPTY; BINS];
        let mut count = [0usize; BINS];
        for &i in items.iter() {
            let b = bin(i);
            bin_box[b] = bin_box[b].union(boxes[i as usize]);
            count[b] += 1;
        }
        let mut right_area = [0f32; BINS];
        let mut right_count = [0usize; BINS];
        let (mut acc, mut n) = (Aabb::EMPTY, 0);
        for b in (1..BINS).rev() {
            acc = acc.union(bin_box[b]);
            n += count[b];
            right_area[b] = acc.half_area();
            right_count[b] = n;
        }
        let (mut acc, mut n) = (Aabb::EMPTY, 0);
        for b in 0..BINS - 1 {
            acc = acc.union(bin_box[b]);
            n += count[b];
            if n == 0 || right_count[b + 1] == 0 {
                continue;
            }
            let cost = acc.half_area() * n as f32 + right_area[b + 1] * right_count[b + 1] as f32;
            if cost < best.0 {
                best = (cost, axis, b);
            }
        }
    }
    if best.0.is_finite() {
        let (_, axis, pos) = best;
        let scale = BINS as f32 / extent[axis];
        let bin = |i: u32| {
            (((boxes[i as usize].center()[axis] - centroids.min[axis]) * scale) as usize)
                .min(BINS - 1)
        };
        // Stable partition keeps the order deterministic.
        let (left, right): (Vec<u32>, Vec<u32>) = items.iter().partition(|&&i| bin(i) <= pos);
        let k = left.len();
        items[..k].copy_from_slice(&left);
        items[k..].copy_from_slice(&right);
        return k;
    }
    items.len() / 2
}
