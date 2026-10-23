//! Static boundary segments (navmesh walls) with a hashed cell index.
use crate::{CrowdError, grid::cell_of, orca::V2};

/// One impassable boundary edge in world space (Y-up metres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wall {
    /// First endpoint.
    pub a: [f32; 3],
    /// Second endpoint.
    pub b: [f32; 3],
}

#[derive(Clone, Copy)]
pub(crate) struct Segment {
    pub a: V2,
    pub b: V2,
    pub y_min: f32,
    pub y_max: f32,
}

/// Immutable wall set for one navmesh revision; build it off the tick thread and
/// share it. Segment order is the caller's order (sort by a stable key first).
pub struct Walls {
    pub(crate) segments: Vec<Segment>,
    inv: f32,
    pub(crate) cell: f32,
    mask: usize,
    starts: Vec<u32>,
    entries: Vec<u32>,
}

fn hash(cx: i32, cz: i32) -> usize {
    ((cx as u32).wrapping_mul(0x8da6_b343) ^ (cz as u32).wrapping_mul(0xd816_3841)) as usize
}

/// Largest number of distinct walls considered per agent query.
pub(crate) const MAX_CANDIDATES: usize = 96;

impl Walls {
    /// Indexes `walls` in cells of `cell` metres. The crowd only sees walls within
    /// one cell of an agent, so use at least the wall look-ahead distance.
    pub fn new(walls: &[Wall], cell: f32) -> Result<Self, CrowdError> {
        if !(cell.is_finite() && cell > 0.0) {
            return Err(CrowdError::new(
                "wall cell size must be finite and positive",
            ));
        }
        let inv = 1.0 / cell;
        let mut segments = Vec::with_capacity(walls.len());
        let mut keyed: Vec<(usize, u32)> = Vec::new();
        let mut cells = Vec::new();
        for (index, wall) in walls.iter().enumerate() {
            if !wall
                .a
                .iter()
                .chain(&wall.b)
                .all(|v| v.is_finite() && v.abs() < 1.0e6)
            {
                return Err(CrowdError::new(format!("wall {index} is not finite")));
            }
            let segment = Segment {
                a: V2::new(wall.a[0], wall.a[2]),
                b: V2::new(wall.b[0], wall.b[2]),
                y_min: wall.a[1].min(wall.b[1]),
                y_max: wall.a[1].max(wall.b[1]),
            };
            let (x0, z0) = cell_of(
                inv,
                segment.a.x.min(segment.b.x),
                segment.a.z.min(segment.b.z),
            );
            let (x1, z1) = cell_of(
                inv,
                segment.a.x.max(segment.b.x),
                segment.a.z.max(segment.b.z),
            );
            if (i64::from(x1) - i64::from(x0) + 1) * (i64::from(z1) - i64::from(z0) + 1) > 4096 {
                return Err(CrowdError::new(format!(
                    "wall {index} spans more than 4096 cells; split long edges or use larger cells"
                )));
            }
            cells.push((x0, z0, x1, z1));
            segments.push(segment);
        }
        let total: usize = cells
            .iter()
            .map(|(x0, z0, x1, z1)| ((x1 - x0 + 1) * (z1 - z0 + 1)) as usize)
            .sum();
        let buckets = (total * 2).next_power_of_two().max(64);
        let mask = buckets - 1;
        keyed.reserve(total);
        for (index, &(x0, z0, x1, z1)) in cells.iter().enumerate() {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    keyed.push((hash(x, z) & mask, index as u32));
                }
            }
        }
        keyed.sort_unstable();
        keyed.dedup();
        let mut starts = vec![0u32; buckets + 1];
        for &(bucket, _) in &keyed {
            starts[bucket + 1] += 1;
        }
        for b in 0..buckets {
            starts[b + 1] += starts[b];
        }
        Ok(Self {
            segments,
            inv,
            cell,
            mask,
            starts,
            entries: keyed.into_iter().map(|(_, index)| index).collect(),
        })
    }

    /// Number of indexed segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// True when there are no walls.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Distinct wall indices near a point, ascending; returns how many were written.
    pub(crate) fn candidates(&self, x: f32, z: f32, out: &mut [u32; MAX_CANDIDATES]) -> usize {
        let (cx, cz) = cell_of(self.inv, x, z);
        let mut n = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let b = hash(cx.wrapping_add(dx), cz.wrapping_add(dz)) & self.mask;
                for &index in &self.entries[self.starts[b] as usize..self.starts[b + 1] as usize] {
                    if n < MAX_CANDIDATES && !out[..n].contains(&index) {
                        out[n] = index;
                        n += 1;
                    }
                }
            }
        }
        out[..n].sort_unstable();
        n
    }
}

/// Closest point on segment `a`-`b` to `p`.
pub(crate) fn closest(a: V2, b: V2, p: V2) -> V2 {
    let ab = b - a;
    let len_sq = ab.len_sq();
    if len_sq <= 0.0 {
        return a;
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    a + ab.scale(t)
}

/// Parameter along `p`-`q` where it properly crosses `a`-`b`, if it does.
pub(crate) fn crossing(p: V2, q: V2, a: V2, b: V2) -> Option<f32> {
    let r = q - p;
    let s = b - a;
    let denominator = crate::orca::det(r, s);
    if denominator.abs() < 1e-9 {
        return None;
    }
    let t = crate::orca::det(a - p, s) / denominator;
    let u = crate::orca::det(a - p, r) / denominator;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}
