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
        for z in (c[2] - 1).max(0)..=(c[2] + 1).min(15) {
            for y in (c[1] - 1).max(0)..=(c[1] + 1).min(15) {
                for x in (c[0] - 1).max(0)..=(c[0] + 1).min(15) {
                    let k = model::key([x, y, z]);
                    let range = self.grid.offsets[k] as usize..self.grid.offsets[k + 1] as usize;
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
