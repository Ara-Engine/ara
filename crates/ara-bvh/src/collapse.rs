// Ported to Rust from tinybvh (tiny_bvh_base.h at c9be3103cc9693b456b0a0353927c11cac7d710f,
// MBVH::ConvertFrom, DecodeWalk and DecodeEmit); Ara adds the depth-bounded grouping
// used when the SAH-optimal tree would overflow the traversal stack.
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

//! Binary to eight-wide collapse. The SAH-optimal distribution of each subtree over
//! up to eight child slots comes from a bottom-up dynamic program (Ylitie et al.
//! 2017, section 4.2), with subtrees of at most three references allowed to merge
//! into one leaf slot. If that tree is deeper than the traversal stack allows, a
//! breadth-first grouping (each wide node takes at least three binary levels) is
//! used instead.
use crate::sbvh::{Binary, MAX_LEAF};
use crate::{Aabb, TRACE_STACK};
use std::collections::VecDeque;

const WIDTH: usize = 8;
const C_TRAV: f32 = 1.0;
const C_INT: f32 = 1.0;
const C_LEAF: f32 = C_INT * 0.8;

/// One child slot of a wide node.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Child {
    /// Another wide node.
    Node(u32),
    /// References `first..first + count` (at most three).
    Leaf {
        bounds: Aabb,
        first: u32,
        count: u32,
    },
}

/// A wide node: its bounds and two to eight children.
#[derive(Debug, Clone)]
pub(crate) struct WideNode {
    pub bounds: Aabb,
    pub children: Vec<Child>,
}

/// The collapsed tree; `nodes[0]` is the root.
pub(crate) struct Wide {
    pub nodes: Vec<WideNode>,
}

impl Wide {
    /// Wide interior levels on the deepest root-to-leaf path (the root counts one).
    pub(crate) fn depth(&self) -> u32 {
        let mut depth = vec![0u32; self.nodes.len()];
        depth[0] = 1;
        let mut deepest = 1;
        // Children are always appended after their parent.
        for (i, node) in self.nodes.iter().enumerate() {
            for child in &node.children {
                if let Child::Node(c) = child {
                    depth[*c as usize] = depth[i] + 1;
                    deepest = deepest.max(depth[i] + 1);
                }
            }
        }
        deepest
    }
}

/// Collapses `bin` (at least one node) to an eight-wide tree whose traversal needs
/// at most [`TRACE_STACK`] stack entries.
pub(crate) fn collapse(bin: &Binary) -> Wide {
    let wide = optimal(bin);
    if wide.depth() <= TRACE_STACK + 1 {
        return wide;
    }
    grouped(bin)
}

fn leaf_of(bin: &Binary, n: u32) -> Child {
    let node = bin.nodes[n as usize];
    Child::Leaf {
        bounds: node.bounds,
        first: node.first,
        count: node.count,
    }
}

/// The SAH-optimal collapse.
fn optimal(bin: &Binary) -> Wide {
    let nodes = &bin.nodes;
    if nodes[0].is_leaf() {
        return Wide {
            nodes: vec![WideNode {
                bounds: nodes[0].bounds,
                children: vec![leaf_of(bin, 0)],
            }],
        };
    }
    let n = nodes.len();
    let mut cost = vec![0f32; n * WIDTH];
    let mut dist = vec![0u8; n * WIDTH];
    let mut as_leaf = vec![false; n];
    let mut first = vec![0u32; n];
    let mut count = vec![0u32; n];
    // Bottom-up: children always have larger indices than their parent.
    for i in (0..n).rev() {
        let node = nodes[i];
        let base = i * WIDTH;
        let sa = node.bounds.half_area();
        if node.is_leaf() {
            first[i] = node.first;
            count[i] = node.count;
            as_leaf[i] = true;
            cost[base..base + WIDTH].fill(sa * (C_LEAF + C_INT * node.count as f32));
            continue;
        }
        let (l, r) = (node.first as usize, node.first as usize + 1);
        count[i] = count[l] + count[r];
        // Subtrees own contiguous ranges (sbvh::finish), left before right.
        first[i] = first[l].min(first[r]);
        for slots in 2..=WIDTH {
            let mut best = f32::INFINITY;
            let mut best_k = 1u8;
            for k in 1..slots {
                let c = cost[l * WIDTH + k - 1] + cost[r * WIDTH + (slots - k) - 1];
                if c < best {
                    best = c;
                    best_k = k as u8;
                }
            }
            cost[base + slots - 1] = best;
            dist[base + slots - 1] = best_k;
        }
        let c_node = sa * C_TRAV + cost[base + WIDTH - 1];
        let c_leaf = if count[i] <= MAX_LEAF {
            sa * (C_LEAF + C_INT * count[i] as f32)
        } else {
            f32::INFINITY
        };
        if c_leaf < c_node {
            cost[base] = c_leaf;
            as_leaf[i] = true;
        } else {
            cost[base] = c_node;
        }
        // A subtree may always take fewer slots than it is offered (dist 0).
        for slots in 2..=WIDTH {
            if cost[base] < cost[base + slots - 1] {
                cost[base + slots - 1] = cost[base];
                dist[base + slots - 1] = 0;
            }
        }
    }
    let mut out = Wide { nodes: Vec::new() };
    let mut queue = VecDeque::from([(0u32, 0u32)]);
    out.nodes.push(WideNode {
        bounds: nodes[0].bounds,
        children: Vec::new(),
    });
    // Breadth-first decode, so each node's children follow it.
    while let Some((src, slot)) = queue.pop_front() {
        let mut tasks = vec![(src, WIDTH)];
        let mut children = Vec::new();
        while let Some((c, slots)) = tasks.pop() {
            let node = nodes[c as usize];
            let k = dist[c as usize * WIDTH + slots - 1] as usize;
            if slots > 1 && !node.is_leaf() && k != 0 {
                tasks.push((node.first + 1, slots - k));
                tasks.push((node.first, k));
                continue;
            }
            if as_leaf[c as usize] {
                children.push(Child::Leaf {
                    bounds: node.bounds,
                    first: first[c as usize],
                    count: count[c as usize],
                });
            } else {
                let index = out.nodes.len() as u32;
                out.nodes.push(WideNode {
                    bounds: node.bounds,
                    children: Vec::new(),
                });
                queue.push_back((c, index));
                children.push(Child::Node(index));
            }
        }
        out.nodes[slot as usize].children = children;
    }
    out
}

/// Breadth-first grouping: each wide node expands its binary frontier level by level
/// until eight slots are full, so interior children lie at least three binary levels
/// down and the wide depth is at most a third of the binary depth (rounded up).
fn grouped(bin: &Binary) -> Wide {
    let nodes = &bin.nodes;
    if nodes[0].is_leaf() {
        return optimal(bin);
    }
    let mut out = Wide {
        nodes: vec![WideNode {
            bounds: nodes[0].bounds,
            children: Vec::new(),
        }],
    };
    let mut pending = VecDeque::from([(0u32, 0u32)]);
    while let Some((src, slot)) = pending.pop_front() {
        let root = nodes[src as usize];
        let mut queue = VecDeque::from([root.first, root.first + 1]);
        let mut done = Vec::new();
        while let Some(c) = queue.pop_front() {
            let node = nodes[c as usize];
            if !node.is_leaf() && queue.len() + done.len() + 2 <= WIDTH {
                queue.push_back(node.first);
                queue.push_back(node.first + 1);
            } else {
                done.push(c);
            }
        }
        let mut children = Vec::with_capacity(done.len());
        for c in done {
            if nodes[c as usize].is_leaf() {
                children.push(leaf_of(bin, c));
            } else {
                let index = out.nodes.len() as u32;
                out.nodes.push(WideNode {
                    bounds: nodes[c as usize].bounds,
                    children: Vec::new(),
                });
                pending.push_back((c, index));
                children.push(Child::Node(index));
            }
        }
        out.nodes[slot as usize].children = children;
    }
    out
}
