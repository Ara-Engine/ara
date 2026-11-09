//! Same S8 neighbour order and arithmetic; cell-sorted Jacobi data avoids ID gathers.
use crate::model::{self, DT, Grid};
use glam::Vec4;

/// Cell-sorted snapshot columns, retaining reference ID accumulation order.
pub struct SortedGrid {
    grid: Grid,
    p: Vec<Vec4>,
    v: Vec<Vec4>,
}

impl SortedGrid {
    /// Allocates scratch storage once, outside timed frames.
    pub fn new(n: usize) -> Self {
        Self {
            grid: Grid::new(n),
            p: vec![Vec4::ZERO; n],
            v: vec![Vec4::ZERO; n],
        }
    }
    /// Rebuilds the deterministic grid and scatters Jacobi data contiguously.
    pub fn build(&mut self, p: &[Vec4], v: &[Vec4]) {
        self.grid.build(p);
        for (row, &id) in self.grid.ids.iter().enumerate() {
            self.p[row] = p[id as usize];
            self.v[row] = v[id as usize];
        }
    }
    /// Computes one boid with the same arithmetic and neighbour order as S8.
    pub fn boid(&self, id: usize, positions: &[Vec4], velocities: &[Vec4]) -> (Vec4, Vec4) {
        let p = positions[id];
        let mut v = velocities[id];
        let mut separation = Vec4::ZERO;
        let mut alignment = Vec4::ZERO;
        let mut cohesion = Vec4::ZERO;
        let mut count = 0;
        let c = model::cell(p);
        // Conservative y/z distance bounds reject whole x spans. Widen each
        // plane because cell() rounds p+32 before flooring; never widen radius.
        let gaps = [1, 2].map(|axis| {
            let lower = c[axis] as f32 * 4.0 - 32.0;
            let padding = 4.0 * f32::EPSILON * 64.0;
            let left = (p[axis] - lower - padding).max(0.0);
            let right = (lower + 4.0 - p[axis] - padding).max(0.0);
            [left * left, 0.0, right * right]
        });
        for z in (c[2] - 1).max(0)..=(c[2] + 1).min(15) {
            for y in (c[1] - 1).max(0)..=(c[1] + 1).min(15) {
                if gaps[0][(y - c[1] + 1) as usize] + gaps[1][(z - c[2] + 1) as usize] >= 16.0 {
                    continue;
                }
                // Adjacent x cells are consecutive in the stable counting sort.
                // One span preserves the exact z/y/x/ID accumulation order.
                let first = model::key([(c[0] - 1).max(0), y, z]);
                let last = model::key([(c[0] + 1).min(15), y, z]) + 1;
                let range = self.grid.offsets[first] as usize..self.grid.offsets[last] as usize;
                for (&other_p, &other_v) in self.p[range.clone()].iter().zip(&self.v[range]) {
                    let d = p - other_p;
                    let d2 = (d.x * d.x + d.y * d.y) + d.z * d.z;
                    if d2 < 16.0 && d2 > 0.0 {
                        separation += d / d2.max(0.01);
                        alignment += other_v;
                        cohesion += other_p;
                        count += 1;
                    }
                }
            }
        }
        if count > 0 {
            let inv = 1.0 / count as f32;
            let acceleration =
                separation * 0.02 + (alignment * inv - v) * 0.05 + (cohesion * inv - p) * 0.01;
            v += (acceleration.truncate() * DT).extend(0.0);
        }
        let speed2 = (v.x * v.x + v.y * v.y) + v.z * v.z;
        if speed2 > 16.0 {
            v *= 4.0 / speed2.sqrt();
        }
        let mut next = p + v * DT;
        for axis in 0..3 {
            if next[axis] >= 32.0 {
                next[axis] -= 64.0;
            }
            if next[axis] < -32.0 {
                next[axis] += 64.0;
            }
        }
        (next, v)
    }
}

/// Adversarial cell/radius planes and clamped edges must retain reference bits.
pub fn validate_pruning() {
    let mut positions = Vec::new();
    for plane in [-36.0f32, -32.0, -28.0, -4.0, 0.0, 4.0, 28.0, 32.0, 36.0] {
        for value in [
            plane.next_down(),
            plane,
            plane.next_up(),
            plane - 0.000001,
            plane + 0.000001,
        ] {
            for axis in 0..3 {
                let mut point = Vec4::W;
                point[axis] = value;
                positions.push(point);
            }
        }
    }
    let velocities: Vec<_> = (0..positions.len())
        .map(|id| model::initial(id as u32).1)
        .collect();
    for n in [1, 2, 3, 4, 5, 7, 31, positions.len()] {
        let p = &positions[..n];
        let v = &velocities[..n];
        let mut reference = Grid::new(n);
        reference.build(p);
        let mut candidate = SortedGrid::new(n);
        candidate.build(p, v);
        for id in 0..n {
            let (a, b) = (reference.boid(id, p, v), candidate.boid(id, p, v));
            assert_eq!(
                a.0.to_array().map(f32::to_bits),
                b.0.to_array().map(f32::to_bits),
                "position n={n} id={id}"
            );
            assert_eq!(
                a.1.to_array().map(f32::to_bits),
                b.1.to_array().map(f32::to_bits),
                "velocity n={n} id={id}"
            );
        }
    }
}
