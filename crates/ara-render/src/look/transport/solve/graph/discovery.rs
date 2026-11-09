//! Batch changed boxes without changing their exact segment-crossing union.
use super::*;

struct Node {
    bounds: Region,
    first: usize,
    end: usize,
    right: Option<usize>,
}

/// Temporary hierarchy over event boxes. A2 still owns the persistent link index.
struct Regions<'a> {
    boxes: &'a mut [Region],
    nodes: Vec<Node>,
}

impl Regions<'_> {
    fn build(&mut self, first: usize, end: usize) -> usize {
        let mut bounds = self.boxes[first];
        for region in &self.boxes[first + 1..end] {
            for axis in 0..3 {
                bounds.min[axis] = bounds.min[axis].min(region.min[axis]);
                bounds.max[axis] = bounds.max[axis].max(region.max[axis]);
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            first,
            end,
            right: None,
        });
        if end - first > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| {
                    (f64::from(bounds.max[a]) - f64::from(bounds.min[a]))
                        .total_cmp(&(f64::from(bounds.max[b]) - f64::from(bounds.min[b])))
                })
                .expect("three axes");
            self.boxes[first..end].sort_unstable_by(|a, b| {
                (f64::from(a.min[axis]) + f64::from(a.max[axis]))
                    .total_cmp(&(f64::from(b.min[axis]) + f64::from(b.max[axis])))
            });
            let middle = first + (end - first) / 2;
            self.build(first, middle);
            self.nodes[index].right = Some(self.build(middle, end));
        }
        index
    }

    fn crosses(&self, index: usize, segment: Segment) -> bool {
        let node = &self.nodes[index];
        if !node.bounds.crosses(segment) {
            return false;
        }
        if let Some(right) = node.right {
            self.crosses(index + 1, segment) || self.crosses(right, segment)
        } else {
            self.boxes[node.first..node.end]
                .iter()
                .any(|region| region.crosses(segment))
        }
    }
}

impl TraceGraph {
    /// One persistent-index query plus exact event-box tests replaces repeated
    /// whole-room queries. Duplicate hits never reset existing dependency debt.
    pub fn discover_regions(&mut self, regions: &mut [Region], frame: u64) -> usize {
        if regions.len() <= 8 {
            return regions
                .iter()
                .map(|&region| self.discover(region, frame))
                .sum();
        }
        let count = regions.len();
        let mut regions = Regions {
            boxes: regions,
            nodes: Vec::new(),
        };
        regions.build(0, count);
        let mut found = 0;
        self.index
            .region(&self.links, regions.nodes[0].bounds, |link| {
                if regions.crosses(0, self.links.segment(link)) {
                    let age = self.pending.entry(link).or_insert(frame);
                    *age = (*age).min(frame);
                    found += 1;
                }
            });
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::transport::solve::{PatchTransport, TransportOptions, tests::fixtures};

    #[test]
    fn batch_discovery_preserves_exact_union_and_old_debt() -> Result<()> {
        let case = fixtures::Case {
            name: "door",
            frames: 90,
            event: 8,
            bodies: 0,
            triangles: 108,
            lights: 1,
        };
        let fixture = fixtures::create(&case)?;
        let mut transport = PatchTransport::new(
            &fixture.scene,
            &fixture.textures,
            fixture.bodies,
            TransportOptions {
                directions: 4,
                ..TransportOptions::default()
            },
            None,
        )?;
        let graph = &mut transport.graph;
        let mut regions: Vec<_> = (0..64)
            .map(|i| {
                let min = [-4. + (i % 8) as f32, -0.25, -4. + (i / 8) as f32];
                Region::new(min, [min[0] + 0.25, 1., min[2] + 0.25]).unwrap()
            })
            .collect();
        // Include exact boundary and zero-volume hits, not only broad boxes.
        let endpoint = graph.links.segment(0).start;
        regions.push(Region::new(endpoint, endpoint).unwrap());
        graph.discover(regions[0], 2);
        let old = graph.pending.clone();
        assert!(!old.is_empty());
        for &region in &regions {
            graph.discover(region, 9);
        }
        let expected = graph.pending.clone();
        graph.pending = old.clone();
        graph.discover_regions(&mut regions, 9);
        assert_eq!(graph.pending, expected);
        assert!(old.keys().all(|link| graph.pending[link] == 2));
        graph.discover_regions(&mut regions, 12);
        assert_eq!(
            graph.pending, expected,
            "new events must not reset old debt"
        );
        Ok(())
    }
}
