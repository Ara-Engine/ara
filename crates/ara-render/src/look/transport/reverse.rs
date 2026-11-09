//! Patch transport: `reverse` (A2 track; see `transport` module docs).

// Wave 2 supplies scheduling/repair. This module only discovers exact dependencies.
#![allow(dead_code)]

use super::links::{GeometryKey, LinkError, LinkStore, Segment};

/// Source -> dense forward-link IDs. Receivers are reconstructed from the forward
/// coalescing groups, saving four bytes on every reverse edge.
pub(crate) struct ReverseLinks {
    offsets: Box<[u32]>,
    links: Box<[u32]>,
}

impl ReverseLinks {
    pub fn build(forward: &LinkStore) -> Result<Self, LinkError> {
        if forward.len() > u32::MAX as usize {
            return Err(LinkError::Capacity);
        }
        let mut offsets = vec![0u32; forward.source_count() + 1];
        for &source in &forward.source {
            offsets[source as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut cursor = offsets[..offsets.len() - 1].to_vec();
        let mut links = vec![0; forward.len()];
        for (link, &source) in forward.source.iter().enumerate() {
            links[cursor[source as usize] as usize] = link as u32;
            cursor[source as usize] += 1;
        }
        Ok(Self {
            offsets: offsets.into_boxed_slice(),
            links: links.into_boxed_slice(),
        })
    }
    pub fn dependencies(&self, source: u32) -> &[u32] {
        &self.links
            [self.offsets[source as usize] as usize..self.offsets[source as usize + 1] as usize]
    }
    /// Union of links received by a patch and links sourcing it, with no duplicates.
    pub fn patch_links(&self, forward: &LinkStore, patch: u32, mut visit: impl FnMut(u32)) {
        for link in forward.receiver_links(patch) {
            visit(link);
        }
        for &link in self.dependencies(patch) {
            if forward.receiver(link) != patch {
                visit(link);
            }
        }
    }
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }
    pub fn link_ids(&self) -> &[u32] {
        &self.links
    }
    pub fn resident_bytes(&self) -> usize {
        size_of_val(self.offsets.as_ref()) + size_of_val(self.links.as_ref())
    }
}

/// Inclusive world-space event bounds. New geometry queries its swept/displacement
/// bounds, even if no previous trace saw that geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Region {
    pub min: [f32; 3],
    pub max: [f32; 3],
}
impl Region {
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Option<Self> {
        ((0..3).all(|i| min[i].is_finite() && max[i].is_finite() && min[i] <= max[i]))
            .then_some(Self { min, max })
    }
    fn empty() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }
    fn include(&mut self, p: [f32; 3]) {
        for (i, &v) in p.iter().enumerate() {
            self.min[i] = self.min[i].min(v);
            self.max[i] = self.max[i].max(v);
        }
    }
    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|i| self.min[i].min(other.min[i])),
            max: std::array::from_fn(|i| self.max[i].max(other.max[i])),
        }
    }
    fn overlaps(self, other: Self) -> bool {
        (0..3).all(|i| self.min[i] <= other.max[i] && self.max[i] >= other.min[i])
    }

    /// Double-precision slab test over exact FP32 endpoints; closed intervals retain
    /// grazing/boundary/zero-length links. No quantized ranges or epsilon exclusions.
    pub fn crosses(self, segment: Segment) -> bool {
        let mut low = 0.0f64;
        let mut high = 1.0f64;
        for axis in 0..3 {
            let origin = f64::from(segment.start[axis]);
            let direction = f64::from(segment.end[axis]) - origin;
            if direction == 0.0 {
                if origin < f64::from(self.min[axis]) || origin > f64::from(self.max[axis]) {
                    return false;
                }
            } else {
                let a = (f64::from(self.min[axis]) - origin) / direction;
                let b = (f64::from(self.max[axis]) - origin) / direction;
                low = low.max(a.min(b));
                high = high.min(a.max(b));
                if low > high {
                    return false;
                }
            }
        }
        true
    }
}

#[derive(Clone, Copy)]
struct Node {
    bounds: Region,
    geometry: u64,
}

/// Implicit binary tree over consecutive coalescing groups (normally 64 links
/// per leaf). Both children and leaf ranges are implicit, with no per-link IDs.
/// Bounds/bloom summaries only prune; exact segments/dependency sets decide hits.
/// Worst-case query work is linear (e.g. long links spanning a whole room).
pub(crate) struct CrossingIndex {
    nodes: Box<[Node]>,
    leaves: usize,
    groups_per_leaf: usize,
}

impl CrossingIndex {
    pub fn build(links: &LinkStore) -> Self {
        Self::with_block_size(links, 64)
    }

    /// Target block size rounds up to whole 32-lane coalescing groups. It changes
    /// index cost/query work, never transport resolution or the exact result set.
    pub fn with_block_size(links: &LinkStore, block_size: usize) -> Self {
        assert!(block_size > 0, "crossing index block size must be positive");
        let groups_per_leaf = block_size.div_ceil(super::links::LANES);
        if links.len() == 0 {
            return Self {
                nodes: Box::new([]),
                leaves: 0,
                groups_per_leaf,
            };
        }
        let blocks = links.groups.len().div_ceil(groups_per_leaf);
        let leaves = blocks.next_power_of_two();
        let empty = Node {
            bounds: Region::empty(),
            geometry: 0,
        };
        let mut nodes = vec![empty; leaves * 2];
        for (block, node) in nodes[leaves..leaves + blocks].iter_mut().enumerate() {
            for group in
                block * groups_per_leaf..((block + 1) * groups_per_leaf).min(links.groups.len())
            {
                for (link, segment) in links.group_segments(group as u32) {
                    node.bounds.include(segment.start);
                    node.bounds.include(segment.end);
                    for stamp in links.witnesses.iter(links.witness[link as usize]) {
                        node.geometry |= bloom(stamp.key);
                    }
                }
            }
        }
        for i in (1..leaves).rev() {
            nodes[i] = Node {
                bounds: nodes[i * 2].bounds.union(nodes[i * 2 + 1].bounds),
                geometry: nodes[i * 2].geometry | nodes[i * 2 + 1].geometry,
            };
        }
        Self {
            nodes: nodes.into_boxed_slice(),
            leaves,
            groups_per_leaf,
        }
    }

    /// Exact set of cached query segments crossing a changed region, including short,
    /// blocked, transparent and previously clear links. No <1 m indexing exemption.
    pub fn region(&self, links: &LinkStore, region: Region, mut visit: impl FnMut(u32)) {
        self.visit(
            links,
            |node| node.bounds.overlaps(region),
            |link, segment| {
                if region.crosses(segment) {
                    visit(link);
                }
            },
        );
    }

    /// Exact links whose visibility depended on this primitive, even after deletion.
    /// Bloom collisions only add leaf work; generation stamps are retained in `links`.
    pub fn primitive(&self, links: &LinkStore, key: GeometryKey, mut visit: impl FnMut(u32)) {
        let mask = bloom(key);
        self.visit(
            links,
            |node| node.geometry & mask == mask,
            |link, _segment| {
                if links
                    .witnesses
                    .iter(links.witness[link as usize])
                    .any(|s| s.key == key)
                {
                    visit(link);
                }
            },
        );
    }

    fn visit(
        &self,
        links: &LinkStore,
        accepts: impl Fn(Node) -> bool,
        mut visit: impl FnMut(u32, Segment),
    ) {
        if self.leaves == 0 {
            return;
        }
        // u32 link IDs bound this binary tree to at most 33 levels.
        let mut stack = [0usize; 64];
        let mut length = 1;
        stack[0] = 1;
        while length != 0 {
            length -= 1;
            let index = stack[length];
            if !accepts(self.nodes[index]) {
                continue;
            }
            if index >= self.leaves {
                let first = (index - self.leaves) * self.groups_per_leaf;
                for group in first..(first + self.groups_per_leaf).min(links.groups.len()) {
                    for (link, segment) in links.group_segments(group as u32) {
                        visit(link, segment);
                    }
                }
            } else {
                stack[length] = index * 2 + 1;
                stack[length + 1] = index * 2;
                length += 2;
            }
        }
    }

    /// Includes geometry stamps/set tables: do not hide them in the forward charge.
    pub fn resident_bytes(&self, links: &LinkStore) -> usize {
        size_of_val(self.nodes.as_ref()) + links.witnesses.resident_bytes()
    }
}

fn bloom(key: GeometryKey) -> u64 {
    let mut value = u64::from(key.instance.slot)
        ^ key.instance.generation.rotate_left(32)
        ^ u64::from(key.primitive);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^= value >> 31;
    (1 << (value & 63)) | (1 << ((value >> 6) & 63))
}

/// Exact resident array capacities. Builder scratch, retained charts, old snapshots
/// and future GPU copies must be added by the caller when measuring peak residency.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Memory {
    pub patches: usize,
    pub forward: usize,
    pub reverse: usize,
    pub index: usize,
    pub links: usize,
}
impl Memory {
    pub fn measure(links: &LinkStore, reverse: &ReverseLinks, index: &CrossingIndex) -> Self {
        Self {
            patches: links.patches.resident_bytes(),
            forward: links.forward_bytes(),
            reverse: reverse.resident_bytes(),
            index: index.resident_bytes(links),
            links: links.len(),
        }
    }
    pub fn link_bytes(self) -> usize {
        self.forward + self.reverse + self.index
    }
    pub fn resident_bytes(self) -> usize {
        self.patches + self.link_bytes()
    }
    pub fn index_bytes_per_link(self) -> f64 {
        if self.links == 0 {
            0.
        } else {
            self.index as f64 / self.links as f64
        }
    }
    /// Decimal MB is the stricter interpretation; reports also print MiB explicitly.
    pub fn within_g3_data_lines(self) -> bool {
        self.index_bytes_per_link() <= 8.
            && self.link_bytes() <= 256_000_000
            && self.resident_bytes() <= 512_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::super::links::tests::graph;
    use super::*;

    #[test]
    fn reverse_csr_is_the_exact_forward_transpose() {
        let graph = graph();
        let reverse = ReverseLinks::build(&graph).unwrap();
        for source in 0..graph.source_count() as u32 {
            let expected: Vec<_> = graph
                .source
                .iter()
                .enumerate()
                .filter(|&(_, &s)| s == source)
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(reverse.dependencies(source), expected);
        }
        for patch in 0..graph.patches.len() as u32 {
            let expected: Vec<_> = (0..graph.len() as u32)
                .filter(|&l| graph.receiver(l) == patch || graph.source[l as usize] == patch)
                .collect();
            let mut found = Vec::new();
            reverse.patch_links(&graph, patch, |link| found.push(link));
            found.sort_unstable();
            assert_eq!(found, expected);
        }
        assert_eq!(
            reverse.resident_bytes(),
            4 * (graph.len() + graph.source_count() + 1)
        );
    }

    #[test]
    fn reverse_region_and_primitive_indexes_match_exhaustive_queries() {
        let graph = graph();
        for block_size in [1, 64, 96, 127] {
            let index = CrossingIndex::with_block_size(&graph, block_size);
            for i in 0..120 {
                let x = (i * 17 % 71) as f32 * 0.05 - 0.5;
                let y = (i * 31 % 53) as f32 * 0.05 - 0.3;
                let z = (i * 11 % 19) as f32;
                let region = Region::new([x, y, z], [x + 0.1, y + 0.2, z + 0.5]).unwrap();
                let expected: Vec<_> = (0..graph.len() as u32)
                    .filter(|&l| region.crosses(graph.segment(l)))
                    .collect();
                let mut found = Vec::new();
                index.region(&graph, region, |l| found.push(l));
                found.sort_unstable();
                assert_eq!(found, expected, "block={block_size} query={i}");
            }
            for primitive in 0..100 {
                let key = GeometryKey {
                    instance: crate::look::trace::InstanceKey {
                        slot: 77,
                        generation: 1,
                    },
                    primitive,
                };
                let expected: Vec<_> = (0..graph.len() as u32)
                    .filter(|&l| {
                        graph
                            .witnesses
                            .iter(graph.witness[l as usize])
                            .any(|s| s.key == key)
                    })
                    .collect();
                let mut found = Vec::new();
                index.primitive(&graph, key, |l| found.push(l));
                found.sort_unstable();
                assert_eq!(found, expected);
            }
        }
    }

    #[test]
    fn reverse_slab_boundary_zero_length_and_parallel_cases() {
        let region = Region::new([0.; 3], [1.; 3]).unwrap();
        for (start, end, expected) in [
            ([-1., 0., 0.], [1., 0., 0.], true),
            ([0.; 3], [0.; 3], true),
            ([2.; 3], [2.; 3], false),
            ([-1., 1.01, 0.], [1., 1.01, 0.], false),
            ([-1., 1., 1.], [0., 1., 1.], true),
            ([2.; 3], [-1.; 3], true),
            ([f32::MIN, 0., 0.], [f32::MAX, 0., 0.], true),
        ] {
            assert_eq!(region.crosses(Segment { start, end }), expected);
        }
        assert!(Region::new([f32::NAN; 3], [1.; 3]).is_none());
    }

    #[test]
    fn reverse_memory_includes_witnesses_and_index_is_below_eight_bytes_per_link() {
        let graph = graph();
        let reverse = ReverseLinks::build(&graph).unwrap();
        let index = CrossingIndex::build(&graph);
        let memory = Memory::measure(&graph, &reverse, &index);
        assert_eq!(size_of::<Node>(), 32);
        assert_eq!(
            memory.index,
            size_of_val(index.nodes.as_ref()) + graph.witnesses.resident_bytes()
        );
        assert_eq!(
            memory.resident_bytes(),
            memory.patches + memory.forward + memory.reverse + memory.index
        );
        assert!(memory.index_bytes_per_link() < 8., "{memory:?}");
        assert!(memory.within_g3_data_lines());
        println!(
            "transport fixture: {memory:?}; index B/link={:.4}",
            memory.index_bytes_per_link()
        );
    }

    #[test]
    fn reverse_unique_geometry_cost_can_fail_the_gate_and_is_not_hidden() {
        use super::super::links::{Candidate, GeometryStamp, Visibility};
        let fixture = graph();
        let graph = LinkStore::build(
            std::sync::Arc::clone(&fixture.patches),
            vec![],
            0,
            |receiver| {
                Ok((0..3)
                    .map(|ordinal| Candidate {
                        ordinal,
                        source: 0,
                        weight: 1.,
                        segment: None,
                        visibility: Visibility {
                            transmittance: [0.; 3],
                            geometry: vec![GeometryStamp {
                                key: GeometryKey {
                                    instance: crate::look::trace::InstanceKey {
                                        slot: 1,
                                        generation: 1,
                                    },
                                    primitive: receiver * 3 + ordinal,
                                },
                                generation: 1,
                            }],
                        },
                    })
                    .collect())
            },
        )
        .unwrap();
        let reverse = ReverseLinks::build(&graph).unwrap();
        let index = CrossingIndex::build(&graph);
        let memory = Memory::measure(&graph, &reverse, &index);
        assert!(memory.index_bytes_per_link() > 8.);
        assert!(!memory.within_g3_data_lines());
        let empty = LinkStore::build(
            std::sync::Arc::new(super::super::patches::PatchBuffers::default()),
            vec![],
            0,
            |_| unreachable!(),
        )
        .unwrap();
        let empty_index = CrossingIndex::build(&empty);
        empty_index.region(&empty, Region::new([0.; 3], [1.; 3]).unwrap(), |_| {
            panic!("empty graph returned a link")
        });
        assert_eq!(ReverseLinks::build(&empty).unwrap().offsets(), [0]);
    }
}
