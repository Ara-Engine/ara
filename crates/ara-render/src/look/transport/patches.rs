//! Patch transport: `patches` (A2 track; see `transport` module docs).

// Wave 1 is an opt-in data layer; wave 2's solver owns production call sites.
#![allow(dead_code)]

use crate::look::trace::{InstanceKey, Tombstone as TraceTombstone};
use ara_patch_charts::{Charts, cache::Key};
use std::{collections::BTreeMap, sync::Arc};

/// Stable object-local chart identity; dense upload indices are deliberately separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PatchId {
    /// Stable scene/trace instance ID, never extraction order.
    pub instance: u64,
    /// Increases on rechart, removal/recreation, wake/sleep; never recycled.
    pub incarnation: u32,
    /// Deterministic index in the instance's chart set.
    pub local: u32,
}

/// Captured identity and surface generation. Both must match to reuse a cached link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PatchStamp {
    pub id: PatchId,
    pub generation: u64,
}

/// Compact tombstone for all old chart IDs of one instance, including zero-link patches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tombstone {
    pub instance: u64,
    pub incarnation: u32,
    pub generation: u64,
    pub count: u32,
}

impl Tombstone {
    pub fn contains(self, id: PatchId) -> bool {
        id.instance == self.instance && id.incarnation == self.incarnation && id.local < self.count
    }
    pub fn stamps(self) -> impl Iterator<Item = PatchStamp> {
        (0..self.count).map(move |local| PatchStamp {
            id: PatchId {
                instance: self.instance,
                incarnation: self.incarnation,
                local,
            },
            generation: self.generation,
        })
    }
}

/// Column-major proper rotation and translation. Instance scale belongs in Charts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub rotation: [[f32; 3]; 3],
    pub translation: [f32; 3],
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            rotation: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            translation: [0.; 3],
        }
    }
}

impl Placement {
    fn vector(self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            self.rotation[0][i] * v[0] + self.rotation[1][i] * v[1] + self.rotation[2][i] * v[2]
        })
    }
    fn point(self, p: [f32; 3]) -> [f32; 3] {
        let p = self.vector(p);
        std::array::from_fn(|i| p[i] + self.translation[i])
    }
    fn validate(self) -> Result<(), PatchError> {
        if self
            .rotation
            .iter()
            .flatten()
            .chain(&self.translation)
            .any(|v| !v.is_finite())
        {
            return Err(PatchError::InvalidPlacement);
        }
        for a in 0..3 {
            for b in 0..3 {
                let dot: f32 = (0..3)
                    .map(|i| self.rotation[a][i] * self.rotation[b][i])
                    .sum();
                if (dot - if a == b { 1. } else { 0. }).abs() > 0.0001 {
                    return Err(PatchError::InvalidPlacement);
                }
            }
        }
        let [a, b, c] = self.rotation;
        let determinant = a[0] * (b[1] * c[2] - b[2] * c[1]) - b[0] * (a[1] * c[2] - a[2] * c[1])
            + c[0] * (a[1] * b[2] - a[2] * b[1]);
        if determinant < 0.9999 {
            return Err(PatchError::InvalidPlacement);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PatchError {
    InvalidPlacement,
    ExhaustedGeneration,
    TooManyPatches,
    UnknownInstance,
    InvalidCharts,
}

struct Resident {
    key: Key,
    charts: Arc<Charts>,
    room: u64,
    placement: Placement,
}

#[derive(Default)]
struct Entry {
    incarnation: u32,
    generation: u64,
    trace_key: Option<InstanceKey>,
    resident: Option<Resident>,
}

/// Event-time registry of static/sleeping surfaces. Call `remove` on wake; active
/// bodies never enter snapshots and receive light through wave 2's readout instead.
#[derive(Default)]
pub(crate) struct PatchRegistry {
    entries: BTreeMap<u64, Entry>,
}

impl PatchRegistry {
    /// Publish imported/scaled charts. Returns old-ID tombstones on topology/metric
    /// replacement. A same-key, rigid relocation only increments the surface stamp.
    pub fn publish(
        &mut self,
        instance: u64,
        key: Key,
        charts: Arc<Charts>,
        room: u64,
        placement: Placement,
    ) -> Result<Option<Tombstone>, PatchError> {
        self.publish_identity((instance, None), key, charts, room, placement)
    }

    /// Bind charts to A1's exact live instance key. Slot reuse or mesh replacement
    /// retires the old patch incarnation even when the chart content is identical.
    pub fn publish_traced(
        &mut self,
        instance: u64,
        trace_key: InstanceKey,
        key: Key,
        charts: Arc<Charts>,
        room: u64,
        placement: Placement,
    ) -> Result<Option<Tombstone>, PatchError> {
        self.publish_identity((instance, Some(trace_key)), key, charts, room, placement)
    }

    fn publish_identity(
        &mut self,
        (instance, trace_key): (u64, Option<InstanceKey>),
        key: Key,
        charts: Arc<Charts>,
        room: u64,
        placement: Placement,
    ) -> Result<Option<Tombstone>, PatchError> {
        placement.validate()?;
        if charts.patches.len() > u32::MAX as usize {
            return Err(PatchError::TooManyPatches);
        }
        if charts
            .patches
            .iter()
            .any(|p| placement.point(p.centroid).iter().any(|v| !v.is_finite()))
        {
            return Err(PatchError::InvalidPlacement);
        }
        let entry = self.entries.entry(instance).or_default();
        let replaced =
            entry.resident.as_ref().is_none_or(|r| r.key != key) || entry.trace_key != trace_key;
        if let Some(old) = &entry.resident
            && old.key == key
            && old.charts != charts
        {
            return Err(PatchError::InvalidCharts);
        }
        let tombstone = if replaced {
            Self::tombstone(instance, entry)
        } else {
            None
        };
        let incarnation = if replaced {
            entry
                .incarnation
                .checked_add(1)
                .ok_or(PatchError::ExhaustedGeneration)?
        } else {
            entry.incarnation
        };
        let changed = replaced
            || entry
                .resident
                .as_ref()
                .is_some_and(|r| r.placement != placement);
        let generation = if changed {
            entry
                .generation
                .checked_add(1)
                .ok_or(PatchError::ExhaustedGeneration)?
        } else {
            entry.generation
        };
        entry.incarnation = incarnation;
        entry.generation = generation;
        entry.trace_key = trace_key;
        entry.resident = Some(Resident {
            key,
            charts,
            room,
            placement,
        });
        Ok(tombstone)
    }

    /// Rigid motion preserves IDs, invalidates endpoint generations and keeps chart storage.
    pub fn move_instance(&mut self, instance: u64, placement: Placement) -> Result<(), PatchError> {
        placement.validate()?;
        let entry = self
            .entries
            .get_mut(&instance)
            .ok_or(PatchError::UnknownInstance)?;
        let resident = entry.resident.as_mut().ok_or(PatchError::UnknownInstance)?;
        if resident
            .charts
            .patches
            .iter()
            .any(|p| placement.point(p.centroid).iter().any(|v| !v.is_finite()))
        {
            return Err(PatchError::InvalidPlacement);
        }
        if resident.placement != placement {
            entry.generation = entry
                .generation
                .checked_add(1)
                .ok_or(PatchError::ExhaustedGeneration)?;
            resident.placement = placement;
        }
        Ok(())
    }

    /// Invalidate material/light-transport attributes without changing chart identity.
    pub fn touch(&mut self, instance: u64) -> Result<(), PatchError> {
        let entry = self
            .entries
            .get_mut(&instance)
            .filter(|e| e.resident.is_some())
            .ok_or(PatchError::UnknownInstance)?;
        entry.generation = entry
            .generation
            .checked_add(1)
            .ok_or(PatchError::ExhaustedGeneration)?;
        Ok(())
    }

    /// Tombstone all patches on removal/wake. The identity ledger remains, preventing ABA.
    pub fn remove(&mut self, instance: u64) -> Option<Tombstone> {
        let entry = self.entries.get_mut(&instance)?;
        let tombstone = Self::tombstone(instance, entry);
        entry.resident = None;
        tombstone
    }

    /// Consume A1's removal/topology tombstone. Late delivery cannot retire a new
    /// instance that already replaced the dead slot/generation under the same id.
    pub fn retire_trace(&mut self, dead: &TraceTombstone) -> Option<Tombstone> {
        if self.entries.get(&dead.id)?.trace_key != Some(dead.key) {
            return None;
        }
        self.remove(dead.id)
    }

    fn tombstone(instance: u64, entry: &Entry) -> Option<Tombstone> {
        Some(Tombstone {
            instance,
            incarnation: entry.incarnation,
            generation: entry.generation,
            count: entry.resident.as_ref()?.charts.patches.len() as u32,
        })
    }

    pub fn generation(&self, id: PatchId) -> Option<u64> {
        let entry = self.entries.get(&id.instance)?;
        let resident = entry.resident.as_ref()?;
        (id.incarnation == entry.incarnation && (id.local as usize) < resident.charts.patches.len())
            .then_some(entry.generation)
    }

    /// Deterministic room/instance/local order, independent of arrival or thread order.
    /// The resulting immutable snapshot may outlive registry edits for stale-link repair.
    pub fn snapshot(&self) -> Result<PatchBuffers, PatchError> {
        let mut residents: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(&id, e)| e.resident.as_ref().map(|r| (id, e, r)))
            .collect();
        residents.sort_unstable_by_key(|(id, _, r)| (r.room, *id));
        let count = residents
            .iter()
            .try_fold(0usize, |n, (_, _, r)| n.checked_add(r.charts.patches.len()))
            .ok_or(PatchError::TooManyPatches)?;
        if count > u32::MAX as usize {
            return Err(PatchError::TooManyPatches);
        }
        let mut result = PatchBuffers::default();
        let mut positions: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::with_capacity(count));
        let mut normals: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::with_capacity(count));
        let mut albedo: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::with_capacity(count));
        let mut area = Vec::with_capacity(count);
        let mut material = Vec::with_capacity(count);
        let mut stamps = Vec::with_capacity(count);
        let mut rooms: Vec<RoomRange> = Vec::new();
        for (instance, entry, resident) in residents {
            if resident.charts.patches.is_empty() {
                continue;
            }
            if rooms.last().is_none_or(|r| r.room != resident.room) {
                rooms.push(RoomRange {
                    room: resident.room,
                    start: stamps.len() as u32,
                    count: 0,
                });
            }
            rooms
                .last_mut()
                .expect("nonempty chart set has a room range")
                .count += resident.charts.patches.len() as u32;
            for (local, patch) in resident.charts.patches.iter().enumerate() {
                let p = resident.placement.point(patch.centroid);
                let n = resident.placement.vector(patch.normal);
                for axis in 0..3 {
                    positions[axis].push(p[axis]);
                    normals[axis].push(n[axis]);
                    albedo[axis].push(patch.albedo[axis]);
                }
                area.push(patch.area);
                material.push(patch.material);
                stamps.push(PatchStamp {
                    id: PatchId {
                        instance,
                        incarnation: entry.incarnation,
                        local: local as u32,
                    },
                    generation: entry.generation,
                });
            }
        }
        let mut by_id: Vec<u32> = (0..count as u32).collect();
        by_id.sort_unstable_by_key(|&i| stamps[i as usize].id);
        result.position = positions.map(Vec::into_boxed_slice);
        result.normal = normals.map(Vec::into_boxed_slice);
        result.albedo = albedo.map(Vec::into_boxed_slice);
        result.area = area.into_boxed_slice();
        result.material = material.into_boxed_slice();
        result.stamps = stamps.into_boxed_slice();
        result.rooms = rooms.into_boxed_slice();
        result.by_id = by_id.into_boxed_slice();
        Ok(result)
    }
}

/// One contiguous room range. GPU solvers dispatch only the active ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RoomRange {
    pub room: u64,
    pub start: u32,
    pub count: u32,
}

/// FP32 structure of arrays; no float/ID quantization. Stable IDs map to dense u32s
/// only within this immutable snapshot, so insertion cannot silently retarget old links.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct PatchBuffers {
    pub position: [Box<[f32]>; 3],
    pub normal: [Box<[f32]>; 3],
    pub albedo: [Box<[f32]>; 3],
    pub area: Box<[f32]>,
    pub material: Box<[u32]>,
    pub stamps: Box<[PatchStamp]>,
    pub rooms: Box<[RoomRange]>,
    by_id: Box<[u32]>,
}

impl PatchBuffers {
    pub fn len(&self) -> usize {
        self.stamps.len()
    }
    pub fn position(&self, index: u32) -> [f32; 3] {
        self.position.each_ref().map(|axis| axis[index as usize])
    }
    pub fn index(&self, id: PatchId) -> Option<u32> {
        self.by_id
            .binary_search_by_key(&id, |&i| self.stamps[i as usize].id)
            .ok()
            .map(|i| self.by_id[i])
    }
    pub fn resident_bytes(&self) -> usize {
        self.position
            .iter()
            .chain(&self.normal)
            .chain(&self.albedo)
            .map(|v| size_of_val(v.as_ref()))
            .sum::<usize>()
            + size_of_val(self.area.as_ref())
            + size_of_val(self.material.as_ref())
            + size_of_val(self.stamps.as_ref())
            + size_of_val(self.rooms.as_ref())
            + size_of_val(self.by_id.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_patch_charts::{Mesh, Settings, Triangle, cache};

    pub(super) fn chart(scale: f32) -> (Key, Arc<Charts>) {
        let mesh = Mesh {
            positions: &[[0., 0., 0.], [0.5, 0., 0.], [0., 0.5, 0.]],
            triangles: &[Triangle {
                vertices: [0, 1, 2],
                material: 0,
                albedo: [0.5; 3],
            }],
        };
        let prepared = cache::prepare(
            mesh,
            Settings {
                scale: [scale; 3],
                ..Settings::default()
            },
            None,
        )
        .unwrap();
        (prepared.key, Arc::new(prepared.charts))
    }

    #[test]
    fn patches_identity_stability_room_order_and_tombstones() {
        let mut a = PatchRegistry::default();
        let mut b = PatchRegistry::default();
        for id in [30, 10, 20] {
            let (k, c) = chart(1.);
            a.publish(id, k, c, id % 20, Placement::default()).unwrap();
        }
        for id in [20, 10, 30] {
            let (k, c) = chart(1.);
            b.publish(id, k, c, id % 20, Placement::default()).unwrap();
        }
        assert_eq!(a.snapshot().unwrap(), b.snapshot().unwrap());
        let before = a.snapshot().unwrap();
        a.move_instance(
            20,
            Placement {
                translation: [2., 0., 0.],
                ..Placement::default()
            },
        )
        .unwrap();
        let (k, c) = chart(1.);
        a.publish(5, k, c, 0, Placement::default()).unwrap();
        for stamp in before.stamps.iter().filter(|s| s.id.instance == 10) {
            assert_eq!(a.generation(stamp.id), Some(stamp.generation));
        }
        for stamp in before.stamps.iter().filter(|s| s.id.instance == 20) {
            assert_eq!(a.generation(stamp.id), Some(stamp.generation + 1));
        }
        let (k, c) = chart(2.);
        let tombstone = a
            .publish(10, k, c, 10, Placement::default())
            .unwrap()
            .unwrap();
        for stamp in before.stamps.iter().filter(|s| s.id.instance == 10) {
            assert!(tombstone.contains(stamp.id));
            assert_eq!(a.generation(stamp.id), None);
        }
        let wake = a.remove(10).unwrap();
        let (k, c) = chart(2.);
        a.publish(10, k, c, 10, Placement::default()).unwrap();
        assert!(wake.stamps().all(|s| a.generation(s.id).is_none()));
    }

    #[test]
    fn patches_rigid_placement_rejects_scale_and_buffers_account_all_arrays() {
        let mut registry = PatchRegistry::default();
        let (k, c) = chart(1.);
        let mut placement = Placement::default();
        placement.rotation[0][0] = 2.;
        assert_eq!(
            registry.publish(1, k, c, 0, placement),
            Err(PatchError::InvalidPlacement)
        );
        let (k, c) = chart(1.);
        registry.publish(1, k, c, 0, Placement::default()).unwrap();
        let snapshot = registry.snapshot().unwrap();
        assert_eq!(
            snapshot.resident_bytes(),
            snapshot.len() * (48 + size_of::<PatchStamp>()) + size_of::<RoomRange>()
        );
        for (i, s) in snapshot.stamps.iter().enumerate() {
            assert_eq!(snapshot.index(s.id), Some(i as u32));
        }
    }

    #[test]
    fn patches_trace_slot_reuse_and_late_tombstones_cannot_alias() {
        use crate::look::trace::Death;
        let mut registry = PatchRegistry::default();
        let (key, charts) = chart(1.);
        let old = InstanceKey {
            slot: 4,
            generation: 2,
        };
        let new = InstanceKey {
            slot: 9,
            generation: 2,
        };
        registry
            .publish_traced(12, old, key, Arc::clone(&charts), 0, Placement::default())
            .unwrap();
        let before = registry.snapshot().unwrap();
        let retired = registry
            .publish_traced(12, new, key, charts, 0, Placement::default())
            .unwrap()
            .unwrap();
        assert!(
            before
                .stamps
                .iter()
                .all(|s| retired.contains(s.id) && registry.generation(s.id).is_none())
        );
        let dead = TraceTombstone {
            epoch: 10,
            id: 12,
            key: old,
            mesh: ara_bvh::MeshHash([0; 32]),
            death: Death::Removed,
        };
        assert_eq!(registry.retire_trace(&dead), None);
        assert!(!registry.snapshot().unwrap().stamps.is_empty());
        assert!(
            registry
                .retire_trace(&TraceTombstone { key: new, ..dead })
                .is_some()
        );
        assert!(registry.snapshot().unwrap().stamps.is_empty());
    }

    #[test]
    fn patches_source_topology_replacement_tombstones_every_old_identity() {
        let mut registry = PatchRegistry::default();
        let (key, charts) = chart(1.);
        registry
            .publish(1, key, charts, 0, Placement::default())
            .unwrap();
        let old = registry.snapshot().unwrap();
        let mesh = Mesh {
            positions: &[[0., 0., 0.], [0.5, 0., 0.], [0., 0.5, 0.], [0.5, 0.5, 0.]],
            triangles: &[
                Triangle {
                    vertices: [0, 1, 2],
                    material: 0,
                    albedo: [0.5; 3],
                },
                Triangle {
                    vertices: [1, 3, 2],
                    material: 0,
                    albedo: [0.5; 3],
                },
            ],
        };
        let prepared = cache::prepare(mesh, Settings::default(), None).unwrap();
        let removed = registry
            .publish(
                1,
                prepared.key,
                Arc::new(prepared.charts),
                0,
                Placement::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(removed.count as usize, old.len());
        assert!(
            old.stamps
                .iter()
                .all(|s| removed.contains(s.id) && registry.generation(s.id).is_none())
        );
        assert!(
            registry
                .snapshot()
                .unwrap()
                .stamps
                .iter()
                .all(|s| !removed.contains(s.id))
        );
    }
}
