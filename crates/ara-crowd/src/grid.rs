//! Uniform hash grid rebuilt with a counting sort; bucket order is input order.

/// Point indices bucketed by XZ cell. After `build`, `order()` lists points grouped
/// by bucket, and `around` yields ranges into that order, so callers can copy their
/// data into bucket order for cache locality.
#[derive(Default)]
pub(crate) struct Grid {
    inv: f32,
    mask: usize,
    starts: Vec<u32>,
    fill: Vec<u32>,
    keys: Vec<u32>,
    entries: Vec<u32>,
}

/// Integer cell coordinate of a world-space XZ position.
pub(crate) fn cell_of(inv: f32, x: f32, z: f32) -> (i32, i32) {
    ((x * inv).floor() as i32, (z * inv).floor() as i32)
}

fn hash(cx: i32, cz: i32) -> usize {
    ((cx as u32).wrapping_mul(0x8da6_b343) ^ (cz as u32).wrapping_mul(0xd816_3841)) as usize
}

impl Grid {
    /// Rebuilds buckets for `count` points; `point(i)` yields XZ. Reuses storage.
    pub(crate) fn build(&mut self, cell: f32, count: usize, point: impl Fn(usize) -> (f32, f32)) {
        let buckets = (count * 2).next_power_of_two().max(64);
        self.inv = 1.0 / cell;
        self.mask = buckets - 1;
        self.starts.clear();
        self.starts.resize(buckets + 1, 0);
        self.keys.clear();
        for i in 0..count {
            let (x, z) = point(i);
            let (cx, cz) = cell_of(self.inv, x, z);
            let bucket = hash(cx, cz) & self.mask;
            self.keys.push(bucket as u32);
            self.starts[bucket + 1] += 1;
        }
        for b in 0..buckets {
            self.starts[b + 1] += self.starts[b];
        }
        self.fill.clear();
        self.fill.extend_from_slice(&self.starts[..buckets]);
        self.entries.clear();
        self.entries.resize(count, 0);
        for (i, &bucket) in self.keys.iter().enumerate() {
            let slot = &mut self.fill[bucket as usize];
            self.entries[*slot as usize] = i as u32;
            *slot += 1;
        }
    }

    /// Original point indices in bucket order.
    pub(crate) fn order(&self) -> &[u32] {
        &self.entries
    }

    /// Visits each distinct bucket covering the 3x3 cells around a point, nearest
    /// cell first, with its range in `order()` coordinates and the squared distance
    /// from the point to its nearest covered cell. `visit` returns the squared
    /// distance beyond which nothing is wanted; farther buckets are skipped.
    /// Hash collisions can add unrelated points; callers filter by distance.
    pub(crate) fn around(
        &self,
        x: f32,
        z: f32,
        mut visit: impl FnMut(std::ops::Range<usize>) -> f32,
    ) {
        let (cx, cz) = cell_of(self.inv, x, z);
        let cell = 1.0 / self.inv;
        let mut buckets = [(0.0f32, 0usize); 9];
        let mut n = 0;
        for dz in -1..=1i32 {
            for dx in -1..=1i32 {
                let (gx, gz) = (cx.wrapping_add(dx), cz.wrapping_add(dz));
                let gap = |p: f32, g: i32| {
                    let lo = g as f32 * cell;
                    (lo - p).max(p - (lo + cell)).max(0.0)
                };
                let (ex, ez) = (gap(x, gx), gap(z, gz));
                let d = ex * ex + ez * ez;
                let b = hash(gx, gz) & self.mask;
                match buckets[..n].iter_mut().find(|e| e.1 == b) {
                    Some(e) => e.0 = e.0.min(d),
                    None => {
                        buckets[n] = (d, b);
                        n += 1;
                    }
                }
            }
        }
        buckets[..n].sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut limit = f32::INFINITY;
        for &(d, b) in &buckets[..n] {
            if d > limit {
                break;
            }
            limit = visit(self.starts[b] as usize..self.starts[b + 1] as usize);
        }
    }
}
