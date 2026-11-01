//! Patch transport: `links` (A2 track; see `transport` module docs).

// Opt-in wave-1 data API. Production use belongs to the wave-2 solver.
#![allow(dead_code)]

use super::patches::{PatchBuffers, PatchId};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(crate) const LANES: usize = 32;
const SET_BIT: u32 = 1 << 31;

/// Narrow bridge to trace's stable source primitive identity. `primitive` is the
/// original triangle index, never the BLAS's reordered storage index. The adapter
/// must supply both topology incarnation and geometry/instance generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct GeometryKey {
    pub instance: u64,
    pub incarnation: u32,
    pub primitive: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GeometryStamp {
    pub key: GeometryKey,
    pub generation: u32,
}

/// A cached query's exact finite segment. Arbitrary sample endpoints are supported;
/// centroid-to-centroid segments are implicit and require no per-link storage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Segment {
    pub start: [f32; 3],
    pub end: [f32; 3],
}
impl Segment {
    pub fn valid(self) -> bool {
        self.start.iter().chain(&self.end).all(|v| v.is_finite())
    }
}

/// Non-patch source: e.g. one finite sun/sky boundary sample or an emitter sample.
/// The caller owns source shape/direction selection; the link layout imposes none.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExternalSource {
    pub id: u64,
    pub generation: u32,
    pub position: [f32; 3],
}

/// Visibility established by tracing. Zero transmission is stored too, so removing
/// a blocker can repair its formerly blocked links. Record every relied-on primitive.
#[derive(Clone, Debug)]
pub(crate) struct Visibility {
    pub transmittance: [f32; 3],
    pub geometry: Vec<GeometryStamp>,
}
impl Visibility {
    pub fn clear() -> Self {
        Self {
            transmittance: [1.; 3],
            geometry: Vec::new(),
        }
    }
}

/// Trace adapter boundary. Alpha/transmission and ray epsilon policy belong to
/// the visibility service; tests inject exact visibility without a GPU or solver.
pub(crate) trait VisibilityQuery {
    fn visibility(&mut self, segment: Segment) -> Result<Visibility, LinkError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkError {
    InvalidEndpoint,
    InvalidVisibility,
    DuplicateOrdinal,
    ConflictingGeneration,
    Capacity,
}

/// One receiver's neutral link sample. Ordinals are arbitrary stable caller labels,
/// not globally shared direction indices. Different receivers can use different sets.
pub(crate) struct Candidate {
    pub ordinal: u32,
    /// `[0, patches.len())` addresses patches; the remaining IDs address externals.
    pub source: u32,
    pub weight: f32,
    pub visibility: Visibility,
    /// None uses the two endpoint centroids; Some preserves exact query endpoints.
    pub segment: Option<Segment>,
}

impl Candidate {
    pub fn traced(
        ordinal: u32,
        source: u32,
        weight: f32,
        segment: Segment,
        query: &mut impl VisibilityQuery,
    ) -> Result<Self, LinkError> {
        if !segment.valid() {
            return Err(LinkError::InvalidEndpoint);
        }
        Ok(Self {
            ordinal,
            source,
            weight,
            visibility: query.visibility(segment)?,
            segment: Some(segment),
        })
    }
}

/// Slot-major, packed ELL group. Lane presence masks eliminate padding for ragged
/// receiver degrees. At each slot, adjacent active receivers read adjacent records.
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct Group {
    pub first: u32,
    pub lanes: u32,
    pub tile: u32,
}

#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct Tile {
    pub first_receiver: u32,
    pub receiver_count: u32,
    pub first_group: u32,
    pub group_count: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SegmentOverride {
    pub link: u32,
    pub segment: Segment,
}

/// Generations are stored once per referenced primitive, with lossless interned sets.
/// Witness 0 is empty, 1..2^31 are single primitive refs, high-bit entries are sets.
#[derive(Default)]
pub(crate) struct Witnesses {
    geometry: Vec<GeometryStamp>,
    offsets: Vec<u32>,
    members: Vec<u32>,
}

impl Witnesses {
    pub fn iter(&self, witness: u32) -> impl Iterator<Item = &GeometryStamp> {
        let single = (witness != 0 && witness & SET_BIT == 0).then(|| witness - 1);
        let range = if witness & SET_BIT != 0 {
            let set = (witness & !SET_BIT) as usize;
            self.offsets[set] as usize..self.offsets[set + 1] as usize
        } else {
            0..0
        };
        single
            .into_iter()
            .chain(self.members[range].iter().copied())
            .map(|i| &self.geometry[i as usize])
    }
    pub fn resident_bytes(&self) -> usize {
        self.geometry.capacity() * size_of::<GeometryStamp>()
            + (self.offsets.capacity() + self.members.capacity()) * 4
    }
}

#[derive(Default)]
struct WitnessBuilder {
    geometry: BTreeMap<GeometryStamp, u32>,
    generations: BTreeMap<GeometryKey, u32>,
    sets: BTreeMap<Vec<u32>, u32>,
    pool: Witnesses,
}

impl WitnessBuilder {
    fn intern(&mut self, stamps: &mut Vec<GeometryStamp>) -> Result<u32, LinkError> {
        stamps.sort_unstable();
        stamps.dedup();
        let mut members = Vec::with_capacity(stamps.len());
        for &stamp in stamps.iter() {
            if let Some(&generation) = self.generations.get(&stamp.key)
                && generation != stamp.generation
            {
                return Err(LinkError::ConflictingGeneration);
            }
            self.generations.insert(stamp.key, stamp.generation);
            let index = if let Some(&index) = self.geometry.get(&stamp) {
                index
            } else {
                if self.pool.geometry.len() >= (SET_BIT - 1) as usize {
                    return Err(LinkError::Capacity);
                }
                let index = self.pool.geometry.len() as u32;
                self.geometry.insert(stamp, index);
                self.pool.geometry.push(stamp);
                index
            };
            members.push(index);
        }
        match members.as_slice() {
            [] => Ok(0),
            [one] => Ok(*one + 1),
            _ => {
                if let Some(&set) = self.sets.get(&members) {
                    return Ok(SET_BIT | set);
                }
                if self.sets.len() >= SET_BIT as usize
                    || self
                        .pool
                        .members
                        .len()
                        .checked_add(members.len())
                        .is_none_or(|n| n > u32::MAX as usize)
                {
                    return Err(LinkError::Capacity);
                }
                if self.pool.offsets.is_empty() {
                    self.pool.offsets.push(0);
                }
                let set = self.sets.len() as u32;
                self.pool.members.extend_from_slice(&members);
                self.pool.offsets.push(self.pool.members.len() as u32);
                self.sets.insert(members, set);
                Ok(SET_BIT | set)
            }
        }
    }
}

/// Immutable cached graph for a resident sector/snapshot. Its endpoints remain at
/// the positions and generations used to trace, even while the live scene changes.
/// Rebuild/publish affected sectors; never silently reinterpret old dense IDs.
pub(crate) struct LinkStore {
    pub patches: Arc<PatchBuffers>,
    pub external: Box<[ExternalSource]>,
    pub source: Vec<u32>,
    pub weight: Vec<f32>,
    pub transmission: [Vec<f32>; 3],
    pub witness: Vec<u32>,
    pub groups: Vec<Group>,
    pub tiles: Vec<Tile>,
    pub overrides: Vec<SegmentOverride>,
    pub witnesses: Witnesses,
}

impl LinkStore {
    /// Streaming construction: only 32 receiver rows are staged at once. `expected_links`
    /// reserves capacity; it is not a cap or an omission policy. Geometry stamps from all
    /// rows must belong to the same scene snapshot. Rows are sorted by their own ordinals.
    pub fn build(
        patches: Arc<PatchBuffers>,
        external: Vec<ExternalSource>,
        expected_links: usize,
        mut row: impl FnMut(u32) -> Result<Vec<Candidate>, LinkError>,
    ) -> Result<Self, LinkError> {
        if patches
            .len()
            .checked_add(external.len())
            .is_none_or(|n| n > u32::MAX as usize)
            || expected_links > u32::MAX as usize
            || external
                .iter()
                .any(|s| !s.position.iter().all(|v| v.is_finite()))
        {
            return Err(LinkError::InvalidEndpoint);
        }
        let mut ids = BTreeSet::new();
        if external.iter().any(|s| !ids.insert(s.id)) {
            return Err(LinkError::InvalidEndpoint);
        }
        let mut result = Self {
            patches,
            external: external.into_boxed_slice(),
            source: Vec::with_capacity(expected_links),
            weight: Vec::with_capacity(expected_links),
            transmission: std::array::from_fn(|_| Vec::with_capacity(expected_links)),
            witness: Vec::with_capacity(expected_links),
            groups: Vec::new(),
            tiles: Vec::new(),
            overrides: Vec::new(),
            witnesses: Witnesses::default(),
        };
        let mut witnesses = WitnessBuilder::default();
        for room_index in 0..result.patches.rooms.len() {
            let room = result.patches.rooms[room_index];
            for first in (room.start..room.start + room.count).step_by(LANES) {
                let count = (room.start + room.count - first).min(LANES as u32);
                let mut rows = Vec::with_capacity(count as usize);
                for receiver in first..first + count {
                    let mut candidates = row(receiver)?;
                    candidates.sort_unstable_by_key(|c| c.ordinal);
                    if candidates.windows(2).any(|w| w[0].ordinal == w[1].ordinal) {
                        return Err(LinkError::DuplicateOrdinal);
                    }
                    rows.push(candidates.into_iter());
                }
                let first_group =
                    u32::try_from(result.groups.len()).map_err(|_| LinkError::Capacity)?;
                loop {
                    let mut group = Group {
                        first: u32::try_from(result.len()).map_err(|_| LinkError::Capacity)?,
                        lanes: 0,
                        tile: result.tiles.len() as u32,
                    };
                    for (lane, candidates) in rows.iter_mut().enumerate() {
                        if let Some(mut c) = candidates.next() {
                            let receiver = first + lane as u32;
                            let implicit = result.implicit_segment(receiver, c.source)?;
                            let segment = c.segment.unwrap_or(implicit);
                            if !segment.valid() {
                                return Err(LinkError::InvalidEndpoint);
                            }
                            if !c.weight.is_finite()
                                || c.weight < 0.
                                || c.visibility
                                    .transmittance
                                    .iter()
                                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                            {
                                return Err(LinkError::InvalidVisibility);
                            }
                            if result.len() == u32::MAX as usize {
                                return Err(LinkError::Capacity);
                            }
                            let witness = witnesses.intern(&mut c.visibility.geometry)?;
                            if segment != implicit {
                                result.overrides.push(SegmentOverride {
                                    link: result.len() as u32,
                                    segment,
                                });
                            }
                            result.source.push(c.source);
                            result.weight.push(c.weight);
                            result.witness.push(witness);
                            for (axis, &value) in c.visibility.transmittance.iter().enumerate() {
                                result.transmission[axis].push(value);
                            }
                            group.lanes |= 1 << lane;
                        }
                    }
                    if group.lanes == 0 {
                        break;
                    }
                    result.groups.push(group);
                }
                result.tiles.push(Tile {
                    first_receiver: first,
                    receiver_count: count,
                    first_group,
                    group_count: result.groups.len() as u32 - first_group,
                });
            }
        }
        result.witnesses = witnesses.pool;
        // Release capacity estimates before publication; final accounting still uses
        // actual capacities, since shrink_to_fit is permitted to retain spare bytes.
        result.source.shrink_to_fit();
        result.weight.shrink_to_fit();
        result.witness.shrink_to_fit();
        for axis in &mut result.transmission {
            axis.shrink_to_fit();
        }
        result.groups.shrink_to_fit();
        result.tiles.shrink_to_fit();
        result.overrides.shrink_to_fit();
        result.witnesses.geometry.shrink_to_fit();
        result.witnesses.offsets.shrink_to_fit();
        result.witnesses.members.shrink_to_fit();
        Ok(result)
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }
    pub fn source_count(&self) -> usize {
        self.patches.len() + self.external.len()
    }
    pub fn source_position(&self, source: u32) -> Result<[f32; 3], LinkError> {
        if (source as usize) < self.patches.len() {
            Ok(self.patches.position(source))
        } else {
            self.external
                .get(source as usize - self.patches.len())
                .map(|s| s.position)
                .ok_or(LinkError::InvalidEndpoint)
        }
    }
    fn implicit_segment(&self, receiver: u32, source: u32) -> Result<Segment, LinkError> {
        Ok(Segment {
            start: self.patches.position(receiver),
            end: self.source_position(source)?,
        })
    }

    /// Dense link -> receiver without a 4-byte receiver field on every link.
    pub fn receiver(&self, link: u32) -> u32 {
        let group = &self.groups[self.groups.partition_point(|g| g.first <= link) - 1];
        let mut mask = group.lanes;
        for _ in 0..link - group.first {
            mask &= mask - 1;
        }
        self.tiles[group.tile as usize].first_receiver + mask.trailing_zeros()
    }

    pub fn segment(&self, link: u32) -> Segment {
        if let Ok(i) = self.overrides.binary_search_by_key(&link, |s| s.link) {
            self.overrides[i].segment
        } else {
            self.implicit_segment(self.receiver(link), self.source[link as usize])
                .expect("published links have valid endpoints")
        }
    }

    /// Iteration order is this receiver's ordinal order, without padded link records.
    pub fn receiver_links(&self, receiver: u32) -> impl Iterator<Item = u32> + '_ {
        let tile = &self.tiles[self.tiles.partition_point(|t| t.first_receiver <= receiver) - 1];
        let lane = receiver - tile.first_receiver;
        self.groups[tile.first_group as usize..(tile.first_group + tile.group_count) as usize]
            .iter()
            .filter(move |g| g.lanes & (1 << lane) != 0)
            .map(move |g| g.first + (g.lanes & ((1 << lane) - 1)).count_ones())
    }

    /// Clear paths can become blocked by newly introduced geometry. Generation checks
    /// alone cannot detect this: the caller MUST also query the swept-region index.
    pub fn stale(&self, link: u32, current: &impl Generations) -> bool {
        let receiver = self.patches.stamps[self.receiver(link) as usize];
        if current.patch(receiver.id) != Some(receiver.generation) {
            return true;
        }
        let source = self.source[link as usize] as usize;
        if source < self.patches.len() {
            let stamp = self.patches.stamps[source];
            if current.patch(stamp.id) != Some(stamp.generation) {
                return true;
            }
        } else {
            let stamp = self.external[source - self.patches.len()];
            if current.source(stamp.id) != Some(stamp.generation) {
                return true;
            }
        }
        self.witnesses
            .iter(self.witness[link as usize])
            .any(|s| current.geometry(s.key) != Some(s.generation))
    }

    /// Forward fields + coalescing metadata + exact sample overrides. Patch snapshots,
    /// reverse CSR and geometry/index arrays are charged separately, once each.
    pub fn forward_bytes(&self) -> usize {
        (self.source.capacity()
            + self.weight.capacity()
            + self.witness.capacity()
            + self.transmission.iter().map(Vec::capacity).sum::<usize>())
            * 4
            + self.groups.capacity() * size_of::<Group>()
            + self.tiles.capacity() * size_of::<Tile>()
            + self.overrides.capacity() * size_of::<SegmentOverride>()
            + size_of_val(self.external.as_ref())
    }
}

/// Live identity lookup; missing IDs/tombstones are stale, including source deletion.
pub(crate) trait Generations {
    fn patch(&self, id: PatchId) -> Option<u32>;
    fn source(&self, id: u64) -> Option<u32>;
    fn geometry(&self, key: GeometryKey) -> Option<u32>;
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::patches::{PatchRegistry, Placement};
    use super::*;
    use ara_patch_charts::{Mesh, Settings, Triangle, cache};

    pub(crate) fn graph() -> LinkStore {
        let mesh = Mesh {
            positions: &[[0., 0., 0.], [2., 0., 0.], [2., 2., 0.], [0., 2., 0.]],
            triangles: &[
                Triangle {
                    vertices: [0, 1, 2],
                    material: 0,
                    albedo: [0.5; 3],
                },
                Triangle {
                    vertices: [0, 2, 3],
                    material: 0,
                    albedo: [0.5; 3],
                },
            ],
        };
        let mut registry = PatchRegistry::default();
        for instance in [8, 3] {
            let prepared = cache::prepare(mesh, Settings::default(), None).unwrap();
            registry
                .publish(
                    instance,
                    prepared.key,
                    Arc::new(prepared.charts),
                    instance,
                    Placement {
                        translation: [0., 0., instance as f32],
                        ..Placement::default()
                    },
                )
                .unwrap();
        }
        let patches = Arc::new(registry.snapshot().unwrap());
        let count = patches.len() as u32;
        LinkStore::build(
            patches,
            vec![ExternalSource {
                id: 900,
                generation: 2,
                position: [1., 1., 20.],
            }],
            0,
            |receiver| {
                Ok((0..receiver % 9 + 1)
                    .rev()
                    .map(|ordinal| Candidate {
                        ordinal,
                        source: (receiver * 5 + ordinal * 11) % (count + 1),
                        weight: (ordinal + 1) as f32 / 64.,
                        visibility: Visibility {
                            transmittance: if receiver % 11 == 0 {
                                [0.; 3]
                            } else {
                                [1., 0.8, 0.2]
                            },
                            geometry: if ordinal % 3 == 0 {
                                vec![]
                            } else {
                                vec![GeometryStamp {
                                    key: GeometryKey {
                                        instance: 77,
                                        incarnation: 1,
                                        primitive: ordinal % 9,
                                    },
                                    generation: 3,
                                }]
                            },
                        },
                        segment: (receiver % 17 == 0).then_some(Segment {
                            start: [receiver as f32 * 0.01, 0., 0.],
                            end: [1., 2., 3.],
                        }),
                    })
                    .collect())
            },
        )
        .unwrap()
    }

    #[test]
    fn links_ragged_slot_major_layout_is_padding_free_and_room_local() {
        let graph = graph();
        assert_eq!(
            graph.len(),
            (0..graph.patches.len()).map(|r| r % 9 + 1).sum::<usize>()
        );
        for receiver in 0..graph.patches.len() as u32 {
            let links: Vec<_> = graph.receiver_links(receiver).collect();
            assert_eq!(links.len(), (receiver % 9 + 1) as usize);
            for (ordinal, &link) in links.iter().enumerate() {
                assert_eq!(graph.receiver(link), receiver);
                assert_eq!(graph.weight[link as usize], (ordinal + 1) as f32 / 64.);
            }
        }
        assert_eq!(
            (0..32).map(|link| graph.receiver(link)).collect::<Vec<_>>(),
            (0..32).collect::<Vec<_>>()
        );
        for tile in &graph.tiles {
            assert!(
                graph
                    .patches
                    .rooms
                    .iter()
                    .any(|r| tile.first_receiver >= r.start
                        && tile.first_receiver + tile.receiver_count <= r.start + r.count)
            );
        }
    }

    #[derive(Default)]
    struct Current {
        patches: BTreeMap<PatchId, u32>,
        sources: BTreeMap<u64, u32>,
        geometry: BTreeMap<GeometryKey, u32>,
    }
    impl Generations for Current {
        fn patch(&self, id: PatchId) -> Option<u32> {
            self.patches.get(&id).copied()
        }
        fn source(&self, id: u64) -> Option<u32> {
            self.sources.get(&id).copied()
        }
        fn geometry(&self, key: GeometryKey) -> Option<u32> {
            self.geometry.get(&key).copied()
        }
    }

    #[test]
    fn links_stamp_both_ends_and_crossed_geometry_and_retain_blocked_paths() {
        let graph = graph();
        let mut current = Current {
            patches: graph
                .patches
                .stamps
                .iter()
                .map(|s| (s.id, s.generation))
                .collect(),
            sources: graph
                .external
                .iter()
                .map(|s| (s.id, s.generation))
                .collect(),
            geometry: graph
                .witnesses
                .geometry
                .iter()
                .map(|s| (s.key, s.generation))
                .collect(),
        };
        assert!((0..graph.len() as u32).all(|link| !graph.stale(link, &current)));
        let link = (0..graph.len() as u32)
            .find(|&l| graph.witness[l as usize] != 0)
            .unwrap();
        let stamp = *graph
            .witnesses
            .iter(graph.witness[link as usize])
            .next()
            .unwrap();
        current.geometry.remove(&stamp.key);
        assert!(graph.stale(link, &current));
        current.geometry.insert(stamp.key, stamp.generation);
        let receiver = graph.patches.stamps[graph.receiver(link) as usize];
        current.patches.insert(receiver.id, receiver.generation + 1);
        assert!(graph.stale(link, &current));
        current.patches.insert(receiver.id, receiver.generation);
        let source = graph.patches.stamps[graph.source[link as usize] as usize];
        current.patches.remove(&source.id);
        assert!(graph.stale(link, &current));
        assert!(
            graph
                .receiver_links(0)
                .all(|l| graph.transmission.iter().all(|axis| axis[l as usize] == 0.))
        );
        let external_link = graph
            .source
            .iter()
            .position(|&s| s as usize == graph.patches.len())
            .unwrap() as u32;
        current.sources.remove(&900);
        assert!(graph.stale(external_link, &current));
    }

    #[test]
    fn links_visibility_adapter_and_invalid_inputs() {
        struct Visible;
        impl VisibilityQuery for Visible {
            fn visibility(&mut self, _: Segment) -> Result<Visibility, LinkError> {
                Ok(Visibility::clear())
            }
        }
        let segment = Segment {
            start: [0.; 3],
            end: [1.; 3],
        };
        assert_eq!(
            Candidate::traced(0, 0, 0.5, segment, &mut Visible)
                .unwrap()
                .visibility
                .transmittance,
            [1.; 3]
        );
        let graph = graph();
        let result = LinkStore::build(Arc::clone(&graph.patches), vec![], 0, |_| {
            Ok(vec![Candidate {
                ordinal: 0,
                source: u32::MAX,
                weight: 1.,
                visibility: Visibility::clear(),
                segment: None,
            }])
        });
        assert!(matches!(result, Err(LinkError::InvalidEndpoint)));
        let mut witnesses = WitnessBuilder::default();
        let key = GeometryKey {
            instance: 1,
            incarnation: 1,
            primitive: 1,
        };
        witnesses
            .intern(&mut vec![GeometryStamp { key, generation: 1 }])
            .unwrap();
        assert_eq!(
            witnesses.intern(&mut vec![GeometryStamp { key, generation: 2 }]),
            Err(LinkError::ConflictingGeneration)
        );
        let mut set = vec![
            GeometryStamp { key, generation: 1 },
            GeometryStamp {
                key: GeometryKey {
                    primitive: 2,
                    ..key
                },
                generation: 1,
            },
        ];
        let index = witnesses.intern(&mut set).unwrap();
        assert_eq!(witnesses.pool.iter(index).copied().collect::<Vec<_>>(), set);
        assert_eq!(witnesses.intern(&mut set).unwrap(), index);
    }
}
