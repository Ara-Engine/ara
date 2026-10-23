//! Shared-goal flow fields: one Dijkstra per goal, O(1) steering per agent.
//!
//! A field is a single walkable layer over a bounded XZ region (open battlefields,
//! plazas, hordes). Multi-level geometry keeps per-agent navmesh paths.
use crate::CrowdError;
use std::{cmp::Reverse, collections::BinaryHeap};

/// Largest grid accepted (cells), bounding memory to tens of megabytes.
pub const MAX_CELLS: usize = 4 << 20;

/// Walkable cells rasterised from navmesh triangles over a bounded region.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkGrid {
    origin: [f32; 2],
    cell: f32,
    width: usize,
    depth: usize,
    /// 0 = blocked; otherwise a traversal cost multiplier.
    cost: Vec<u8>,
    height: Vec<f32>,
}

impl WalkGrid {
    /// Empty (fully blocked) grid covering `min`..`max` in XZ with square cells.
    pub fn new(min: [f32; 2], max: [f32; 2], cell: f32) -> Result<Self, CrowdError> {
        let ok = min.iter().chain(&max).all(|v| v.is_finite()) && cell.is_finite() && cell > 0.0;
        if !ok || max[0] <= min[0] || max[1] <= min[1] {
            return Err(CrowdError::new(
                "walk grid bounds/cell must be finite and nonempty",
            ));
        }
        let width = ((max[0] - min[0]) / cell).ceil() as usize;
        let depth = ((max[1] - min[1]) / cell).ceil() as usize;
        if width.saturating_mul(depth) > MAX_CELLS {
            return Err(CrowdError::new(format!(
                "walk grid needs {width}x{depth} cells; the limit is {MAX_CELLS}"
            )));
        }
        Ok(Self {
            origin: min,
            cell,
            width,
            depth,
            cost: vec![0; width * depth],
            height: vec![0.0; width * depth],
        })
    }

    /// Marks cells whose centres lie inside triangles (XZ). `cost` scales traversal
    /// (1 = normal). Earlier triangles keep their height when floors overlap.
    pub fn rasterize(&mut self, triangles: &[[[f32; 3]; 3]], cost: u8) {
        let cost = cost.max(1);
        for t in triangles {
            let xs = [t[0][0], t[1][0], t[2][0]];
            let zs = [t[0][2], t[1][2], t[2][2]];
            let lo = |v: [f32; 3]| v.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = |v: [f32; 3]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let x0 = (((lo(xs) - self.origin[0]) / self.cell).floor().max(0.0)) as usize;
            let z0 = (((lo(zs) - self.origin[1]) / self.cell).floor().max(0.0)) as usize;
            let x1 =
                (((hi(xs) - self.origin[0]) / self.cell).ceil().max(0.0) as usize).min(self.width);
            let z1 =
                (((hi(zs) - self.origin[1]) / self.cell).ceil().max(0.0) as usize).min(self.depth);
            for z in z0..z1 {
                for x in x0..x1 {
                    let px = self.origin[0] + (x as f32 + 0.5) * self.cell;
                    let pz = self.origin[1] + (z as f32 + 0.5) * self.cell;
                    if let Some(y) = barycentric_height(t, px, pz) {
                        let i = z * self.width + x;
                        if self.cost[i] == 0 {
                            self.cost[i] = cost;
                            self.height[i] = y;
                        }
                    }
                }
            }
        }
    }

    /// Cell size in metres.
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// Number of walkable cells.
    pub fn walkable_cells(&self) -> usize {
        self.cost.iter().filter(|c| **c > 0).count()
    }

    fn index(&self, x: f32, z: f32) -> Option<usize> {
        let fx = ((x - self.origin[0]) / self.cell).floor();
        let fz = ((z - self.origin[1]) / self.cell).floor();
        if fx < 0.0 || fz < 0.0 || fx >= self.width as f32 || fz >= self.depth as f32 {
            return None;
        }
        Some(fz as usize * self.width + fx as usize)
    }
}

fn barycentric_height(t: &[[f32; 3]; 3], x: f32, z: f32) -> Option<f32> {
    let (ax, az, bx, bz, cx, cz) = (t[0][0], t[0][2], t[1][0], t[1][2], t[2][0], t[2][2]);
    let d = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
    if d.abs() < 1e-12 {
        return None;
    }
    let u = ((bz - cz) * (x - cx) + (cx - bx) * (z - cz)) / d;
    let v = ((cz - az) * (x - cx) + (ax - cx) * (z - cz)) / d;
    let w = 1.0 - u - v;
    (u >= -1e-6 && v >= -1e-6 && w >= -1e-6).then(|| u * t[0][1] + v * t[1][1] + w * t[2][1])
}

/// Distance-to-goal and steering directions for one goal. Immutable once built;
/// share it (`Arc`) between every agent heading to the same place.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowField {
    origin: [f32; 2],
    cell: f32,
    width: usize,
    depth: usize,
    distance: Vec<f32>,
    direction: Vec<[f32; 2]>,
    height: Vec<f32>,
    goal: [f32; 3],
}

const NEIGHBORS: [(isize, isize); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

impl FlowField {
    /// Dijkstra from the goal cell over 8-connected walkable cells (no corner
    /// cutting). Ties pop in cell-index order, so the field is deterministic.
    pub fn build(grid: &WalkGrid, goal: [f32; 3]) -> Result<Self, CrowdError> {
        let start = grid
            .index(goal[0], goal[2])
            .filter(|&i| grid.cost[i] > 0)
            .ok_or_else(|| CrowdError::new("flow-field goal is outside the walkable grid"))?;
        let (w, d) = (grid.width, grid.depth);
        let mut distance = vec![f32::INFINITY; w * d];
        let mut heap = BinaryHeap::new();
        distance[start] = 0.0;
        heap.push(Reverse((0u32, start)));
        let diagonal = std::f32::consts::SQRT_2;
        while let Some(Reverse((bits, i))) = heap.pop() {
            let here = f32::from_bits(bits);
            if here > distance[i] {
                continue;
            }
            let (x, z) = ((i % w) as isize, (i / w) as isize);
            for (dx, dz) in NEIGHBORS {
                let Some(j) = step(grid, x, z, dx, dz) else {
                    continue;
                };
                let length = if dx != 0 && dz != 0 { diagonal } else { 1.0 };
                let cost =
                    length * grid.cell * 0.5 * (f32::from(grid.cost[i]) + f32::from(grid.cost[j]));
                let next = here + cost;
                if next < distance[j] {
                    distance[j] = next;
                    heap.push(Reverse((next.to_bits(), j)));
                }
            }
        }
        let mut direction = vec![[0.0f32; 2]; w * d];
        for i in 0..w * d {
            if !distance[i].is_finite() || i == start {
                continue;
            }
            let (x, z) = ((i % w) as isize, (i / w) as isize);
            let mut best = (distance[i], None);
            for (dx, dz) in NEIGHBORS {
                if let Some(j) = step(grid, x, z, dx, dz)
                    && distance[j] < best.0
                {
                    best = (distance[j], Some((dx, dz)));
                }
            }
            if let Some((dx, dz)) = best.1 {
                let len = ((dx * dx + dz * dz) as f32).sqrt();
                direction[i] = [dx as f32 / len, dz as f32 / len];
            }
        }
        Ok(Self {
            origin: grid.origin,
            cell: grid.cell,
            width: w,
            depth: d,
            distance,
            direction,
            height: grid.height.clone(),
            goal,
        })
    }

    fn index(&self, p: [f32; 3]) -> Option<usize> {
        let fx = ((p[0] - self.origin[0]) / self.cell).floor();
        let fz = ((p[2] - self.origin[1]) / self.cell).floor();
        if fx < 0.0 || fz < 0.0 || fx >= self.width as f32 || fz >= self.depth as f32 {
            return None;
        }
        Some(fz as usize * self.width + fx as usize)
    }

    /// Goal used to build the field.
    pub fn goal(&self) -> [f32; 3] {
        self.goal
    }

    /// Path distance to the goal in metres, or None when unreachable/outside.
    pub fn distance(&self, p: [f32; 3]) -> Option<f32> {
        self.index(p)
            .map(|i| self.distance[i])
            .filter(|d| d.is_finite())
    }

    /// Unit XZ steering direction toward the goal. In the goal cell the direction
    /// points straight at the goal; None outside the reachable area.
    pub fn direction(&self, p: [f32; 3]) -> Option<[f32; 2]> {
        let i = self.index(p)?;
        if !self.distance[i].is_finite() {
            return None;
        }
        if self.distance[i] == 0.0 {
            let (dx, dz) = (self.goal[0] - p[0], self.goal[2] - p[2]);
            let len = (dx * dx + dz * dz).sqrt();
            return Some(if len > 1e-6 {
                [dx / len, dz / len]
            } else {
                [0.0, 0.0]
            });
        }
        Some(self.direction[i])
    }

    /// Surface height of the cell containing `p`, when walkable.
    pub fn height(&self, p: [f32; 3]) -> Option<f32> {
        self.index(p)
            .filter(|&i| self.distance[i].is_finite())
            .map(|i| self.height[i])
    }
}

/// Neighbour index if walkable and, for diagonals, both orthogonal cells are too.
fn step(grid: &WalkGrid, x: isize, z: isize, dx: isize, dz: isize) -> Option<usize> {
    let (w, d) = (grid.width as isize, grid.depth as isize);
    let open = |x: isize, z: isize| {
        (0..w).contains(&x) && (0..d).contains(&z) && grid.cost[(z * w + x) as usize] > 0
    };
    if !open(x + dx, z + dz) || (dx != 0 && dz != 0 && (!open(x + dx, z) || !open(x, z + dz))) {
        return None;
    }
    Some(((z + dz) * w + x + dx) as usize)
}
