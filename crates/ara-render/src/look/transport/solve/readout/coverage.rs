//! Barycentric BVH for large source triangles covered by many 25 cm patches.
//! Traversal preserves the original first-fragment tie rule at shared boundaries.
use super::{Coverage, Vec2};

#[derive(Default)]
pub(super) struct TriangleCoverage {
    pub entries: Vec<Coverage>,
    order: Vec<usize>,
    nodes: Vec<Node>,
}

struct Node {
    min: Vec2,
    max: Vec2,
    first: usize,
    count: usize,
    skip: usize,
}

impl Coverage {
    fn contains(&self, p: Vec2) -> bool {
        let [a, b, d] = self.bary.map(Vec2::from);
        let cross = |u: Vec2, v: Vec2| u.x * v.y - u.y * v.x;
        let area = cross(b - a, d - a);
        if area == 0. {
            return false;
        }
        let u = cross(b - p, d - p) / area;
        let v = cross(d - p, a - p) / area;
        let w = 1. - u - v;
        u >= -2e-5 && v >= -2e-5 && w >= -2e-5
    }

    fn bounds(&self) -> (Vec2, Vec2) {
        let [a, b, c] = self.bary.map(Vec2::from);
        let lo = a.min(b).min(c);
        let hi = a.max(b).max(c);
        let e = b - a;
        let f = c - a;
        let area = (e.x * f.y - e.y * f.x).abs();
        // Include the leaf's barycentric tolerance and a condition-dependent
        // roundoff allowance. Slivers get broad boxes rather than losing hits.
        let slack = 4e-5 + 16. * f32::EPSILON / area.max(f32::MIN_POSITIVE);
        let expansion = (hi - lo) * slack + Vec2::splat(4. * f32::EPSILON);
        (lo - expansion, hi + expansion)
    }
}

impl TriangleCoverage {
    pub fn build(&mut self) {
        self.nodes.clear();
        self.order.clear();
        self.order.extend(0..self.entries.len());
        if !self.order.is_empty() {
            build(&self.entries, &mut self.order, 0, &mut self.nodes);
        }
    }

    pub fn patch(&self, point: Vec2) -> Option<u32> {
        let mut cursor = 0;
        let mut best = usize::MAX;
        while cursor < self.nodes.len() {
            let node = &self.nodes[cursor];
            if point.cmplt(node.min).any() || point.cmpgt(node.max).any() {
                cursor = node.skip;
            } else if node.count == 0 {
                cursor += 1;
            } else {
                for &index in &self.order[node.first..node.first + node.count] {
                    if index < best && self.entries[index].contains(point) {
                        best = index;
                    }
                }
                cursor = node.skip;
            }
        }
        self.entries.get(best).map(|c| c.patch)
    }

    pub fn resident_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Coverage>()
            + self.order.capacity() * size_of::<usize>()
            + self.nodes.capacity() * size_of::<Node>()
    }
}

fn build(entries: &[Coverage], order: &mut [usize], first: usize, nodes: &mut Vec<Node>) {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for &i in order.iter() {
        let (a, b) = entries[i].bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    let slot = nodes.len();
    nodes.push(Node {
        min: lo,
        max: hi,
        first,
        count: order.len(),
        skip: 0,
    });
    if order.len() > 8 {
        let axis = usize::from(hi.y - lo.y > hi.x - lo.x);
        order.sort_unstable_by(|&a, &b| {
            let centre = |i: usize| entries[i].bary.iter().map(|v| v[axis]).sum::<f32>();
            centre(a).total_cmp(&centre(b)).then(a.cmp(&b))
        });
        let middle = order.len() / 2;
        let (left, right) = order.split_at_mut(middle);
        build(entries, left, first, nodes);
        build(entries, right, first + middle, nodes);
        nodes[slot].count = 0;
    }
    nodes[slot].skip = nodes.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_coverage_matches_scan_at_interiors_edges_and_thin_fragments() {
        let mut coverage = TriangleCoverage::default();
        for y in 0..80 {
            for x in 0..80 - y {
                let a = Vec2::new(x as f32, y as f32) / 80.;
                let b = a + Vec2::X / 80.;
                let c = a + Vec2::Y / 80.;
                coverage.entries.push(Coverage {
                    patch: coverage.entries.len() as u32,
                    bary: [a.to_array(), b.to_array(), c.to_array()],
                });
            }
        }
        coverage.entries.push(Coverage {
            patch: 9000,
            bary: [[0., 0.], [1., 0.], [0.5, 1e-7]],
        });
        coverage.build();
        let compare = |p| {
            assert_eq!(
                coverage.patch(p),
                coverage
                    .entries
                    .iter()
                    .find(|c| c.contains(p))
                    .map(|c| c.patch),
                "point {p:?}"
            );
        };
        for entry in &coverage.entries {
            let [a, b, c] = entry.bary.map(Vec2::from);
            for p in [
                a,
                b,
                c,
                (a + b + c) / 3.,
                (a + b) * 0.5,
                a - Vec2::splat(1e-7),
            ] {
                compare(p);
            }
        }
        for i in 0..1000 {
            compare(Vec2::new(
                (i * 7919 % 10000) as f32 / 10000.,
                (i * 3571 % 10000) as f32 / 10000.,
            ));
        }
    }
}
