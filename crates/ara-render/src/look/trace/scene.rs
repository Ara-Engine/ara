//! The CPU model behind `trace()`: one BLAS per mesh hash, instances keyed by the
//! caller's stable ids (A3's feed ids), the threaded TLAS, alpha materials, and the
//! identities transport caches rely on.
//!
//! Identities:
//! - [`BlasKey`]: a BLAS slot and its generation (bumped when the slot is reused).
//! - [`InstanceKey`]: an instance slot (the GPU instance index `trace()` reports) and
//!   its generation, bumped when the slot is reused or the instance's mesh changes
//!   (a topology change); moves, material and mask edits keep it.
//! - [`PrimitiveKey`]: mesh hash, submesh and source triangle: stable for as long as
//!   the mesh bytes are, independent of how the BVH was built.
//!
//! Removing an instance or replacing its mesh records a [`Tombstone`]; callers that
//! cached `(InstanceKey, triangle)` pairs read [`TraceScene::tombstones_since`] (or
//! check [`TraceScene::primitive`]) to find what died.
use anyhow::{Result, bail, ensure};
use ara_bvh::tlas::Tlas;
use ara_bvh::{Aabb, Blas, MeshHash};
use glam::Affine3A;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Material index meaning "opaque, no alpha test".
pub(crate) const NO_MATERIAL: u32 = u32::MAX;

/// A BLAS slot and its generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BlasKey {
    /// Slot in the GPU BLAS table.
    pub slot: u32,
    /// Bumped each time the slot holds a different mesh.
    pub generation: u64,
}

/// An instance slot and its generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InstanceKey {
    /// The instance index `trace()` hits report.
    pub slot: u32,
    /// Bumped when the slot is reused or its mesh is replaced.
    pub generation: u64,
}

/// Stable identity of one triangle of one mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PrimitiveKey {
    /// The mesh content hash.
    pub mesh: MeshHash,
    /// Submesh (material slot) of the triangle.
    pub submesh: u32,
    /// Source triangle index in the mesh.
    pub triangle: u32,
}

/// Why cached ids of an instance died.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Death {
    /// The instance was removed.
    Removed,
    /// The instance now uses another mesh (its old triangles are gone).
    MeshReplaced {
        /// The new mesh.
        mesh: MeshHash,
    },
}

/// A topology tombstone: an instance key that will never be live again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tombstone {
    /// Sequence number (the scene's epoch when it was issued).
    pub epoch: u64,
    /// The caller id of the instance.
    pub id: u64,
    /// The dead key.
    pub key: InstanceKey,
    /// The mesh its cached triangles belonged to.
    pub mesh: MeshHash,
    /// What happened.
    pub death: Death,
}

/// A mesh as `trace()` holds it: its BLAS and, for meshes alpha-tested materials
/// use, the corner UVs of every source triangle (`[uv0 a, b, c, uv1 a, b, c]`).
#[derive(Debug, Clone)]
pub(crate) struct TraceMesh {
    /// The BLAS.
    pub blas: Arc<Blas>,
    /// Corner UVs per source triangle, or `None` (alpha tests then pass).
    pub uvs: Option<Arc<Vec<[f32; 12]>>>,
}

/// An alpha-tested material: a candidate hit is solid where the base colour
/// texture's alpha times `factor` reaches `cutoff` (glTF MASK).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AlphaMaterial {
    /// Sampled-image heap index of the base colour texture.
    pub texture: u32,
    /// UV set (0 or 1) of the texture.
    pub uv_set: u32,
    /// Alpha cutoff.
    pub cutoff: f32,
    /// Base colour alpha factor.
    pub factor: f32,
    /// UV offset after scale and rotation (KHR_texture_transform).
    pub offset: [f32; 2],
    /// UV scale.
    pub scale: [f32; 2],
    /// UV rotation in radians.
    pub rotation: f32,
}

/// One instance.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TraceInstance {
    /// Its mesh (registered with [`TraceScene::add_mesh`]).
    pub mesh: MeshHash,
    /// Object to world.
    pub world: Affine3A,
    /// Instance mask bits; a ray sees the instance when masks share a bit.
    pub mask: u8,
    /// Material index per submesh into the alpha table ([`NO_MATERIAL`]: opaque).
    pub materials: Vec<u32>,
    /// Caller word reported with hits (e.g. the native instance row).
    pub user: u32,
}

#[derive(Debug)]
struct MeshSlot {
    key: BlasKey,
    mesh: TraceMesh,
    users: u32,
}

#[derive(Debug)]
pub(crate) struct InstanceSlot {
    pub id: u64,
    pub key: InstanceKey,
    pub instance: TraceInstance,
    pub bounds: Aabb,
}

/// What [`TraceScene::commit`] did to the TLAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Commit {
    /// Nothing moved or changed.
    Unchanged,
    /// Moves refitted the existing tree.
    Refit,
    /// The tree was rebuilt (structural change, or refits degraded it).
    Rebuilt,
}

/// The scene `trace()` queries. Edits mark it dirty; [`Self::commit`] updates the
/// TLAS, then `TraceGpu::update` uploads.
#[derive(Debug, Default)]
pub(crate) struct TraceScene {
    meshes: BTreeMap<MeshHash, MeshSlot>,
    blas_slots: Vec<Option<MeshHash>>,
    blas_generations: Vec<u64>,
    instances: Vec<Option<InstanceSlot>>,
    instance_generations: Vec<u64>,
    by_id: BTreeMap<u64, u32>,
    alpha: Vec<Option<AlphaMaterial>>,
    tlas: Tlas,
    tombstones: Vec<Tombstone>,
    epoch: u64,
    /// Bumped by changes that need a GPU re-layout (BLAS set, instance slots).
    structure: u64,
    /// Bumped by changes to TLAS boxes or instance records only.
    motion: u64,
    /// Bumped by alpha table or per-instance material edits.
    materials: u64,
    dirty_structure: bool,
    dirty_motion: bool,
}

impl TraceScene {
    /// Registers a mesh (a no-op if its hash is already present; new UVs replace
    /// missing ones). Returns its BLAS key.
    pub(crate) fn add_mesh(&mut self, mesh: TraceMesh) -> BlasKey {
        let hash = mesh.blas.hash;
        if let Some(slot) = self.meshes.get_mut(&hash) {
            if slot.mesh.uvs.is_none() && mesh.uvs.is_some() {
                slot.mesh.uvs = mesh.uvs;
                self.dirty_structure = true;
            }
            return slot.key;
        }
        let slot = match self.blas_slots.iter().position(Option::is_none) {
            Some(s) => s,
            None => {
                self.blas_slots.push(None);
                self.blas_generations.push(0);
                self.blas_slots.len() - 1
            }
        };
        self.blas_slots[slot] = Some(hash);
        self.blas_generations[slot] += 1;
        let key = BlasKey {
            slot: slot as u32,
            generation: self.blas_generations[slot],
        };
        self.meshes.insert(
            hash,
            MeshSlot {
                key,
                mesh,
                users: 0,
            },
        );
        self.dirty_structure = true;
        key
    }

    /// Drops meshes no instance uses (their slots become free for reuse).
    pub(crate) fn remove_unused_meshes(&mut self) {
        let unused: Vec<MeshHash> = self
            .meshes
            .iter()
            .filter(|(_, m)| m.users == 0)
            .map(|(h, _)| *h)
            .collect();
        for hash in unused {
            if let Some(m) = self.meshes.remove(&hash) {
                self.blas_slots[m.key.slot as usize] = None;
                self.dirty_structure = true;
            }
        }
    }

    /// Inserts or replaces instance `id`. Replacing the mesh is a topology change:
    /// the old key dies with a tombstone and the instance gets a new generation.
    pub(crate) fn set_instance(&mut self, id: u64, instance: TraceInstance) -> Result<InstanceKey> {
        let Some(mesh) = self.meshes.get(&instance.mesh) else {
            bail!(
                "trace instance {id} uses an unregistered mesh {:?}",
                instance.mesh
            );
        };
        let submeshes = mesh.mesh.blas.stats.submeshes as usize;
        ensure!(
            instance.materials.len() == submeshes,
            "trace instance {id} has {} materials for {submeshes} submeshes",
            instance.materials.len()
        );
        let bounds = mesh.mesh.blas.bounds.transformed(&instance.world);
        if let Some(&slot) = self.by_id.get(&id) {
            let old = self.instances[slot as usize]
                .as_mut()
                .expect("indexed slot is live");
            if old.instance.mesh != instance.mesh {
                let (dead, old_mesh) = (old.key, old.instance.mesh);
                self.release_mesh(old_mesh);
                self.acquire_mesh(instance.mesh);
                let generation = &mut self.instance_generations[slot as usize];
                *generation += 1;
                let key = InstanceKey {
                    slot,
                    generation: *generation,
                };
                self.bury(
                    id,
                    dead,
                    old_mesh,
                    Death::MeshReplaced {
                        mesh: instance.mesh,
                    },
                );
                let s = self.instances[slot as usize].as_mut().expect("live");
                s.key = key;
                s.instance = instance;
                s.bounds = bounds;
                self.dirty_structure = true;
                return Ok(key);
            }
            if old.instance.world != instance.world {
                self.dirty_motion = true;
            }
            if old.instance.materials != instance.materials
                || old.instance.mask != instance.mask
                || old.instance.user != instance.user
            {
                self.materials += 1;
            }
            old.instance = instance;
            old.bounds = bounds;
            return Ok(old.key);
        }
        self.acquire_mesh(instance.mesh);
        let slot = match self.instances.iter().position(Option::is_none) {
            Some(s) => s,
            None => {
                self.instances.push(None);
                self.instance_generations.push(0);
                self.instances.len() - 1
            }
        };
        self.instance_generations[slot] += 1;
        let key = InstanceKey {
            slot: slot as u32,
            generation: self.instance_generations[slot],
        };
        self.instances[slot] = Some(InstanceSlot {
            id,
            key,
            instance,
            bounds,
        });
        self.by_id.insert(id, slot as u32);
        self.dirty_structure = true;
        Ok(key)
    }

    /// Moves instance `id` (the refit path). Returns false for an unknown id.
    pub(crate) fn set_transform(&mut self, id: u64, world: Affine3A) -> bool {
        let Some(&slot) = self.by_id.get(&id) else {
            return false;
        };
        let s = self.instances[slot as usize]
            .as_mut()
            .expect("indexed slot is live");
        if s.instance.world != world {
            let blas = &self.meshes[&s.instance.mesh].mesh.blas;
            s.bounds = blas.bounds.transformed(&world);
            s.instance.world = world;
            self.dirty_motion = true;
        }
        true
    }

    /// Removes instance `id`, burying its key. Returns false for an unknown id.
    pub(crate) fn remove_instance(&mut self, id: u64) -> bool {
        let Some(slot) = self.by_id.remove(&id) else {
            return false;
        };
        let s = self.instances[slot as usize]
            .take()
            .expect("indexed slot is live");
        self.release_mesh(s.instance.mesh);
        self.bury(id, s.key, s.instance.mesh, Death::Removed);
        self.dirty_structure = true;
        true
    }

    /// Sets alpha material `index` (`None`: opaque).
    pub(crate) fn set_alpha_material(&mut self, index: u32, material: Option<AlphaMaterial>) {
        let i = index as usize;
        if self.alpha.len() <= i {
            self.alpha.resize(i + 1, None);
        }
        if self.alpha[i] != material {
            self.alpha[i] = material;
            self.materials += 1;
        }
    }

    /// Applies pending edits to the TLAS: rebuild on structural change (or when
    /// refits degraded it), refit on moves.
    pub(crate) fn commit(&mut self) -> Commit {
        let structural = std::mem::take(&mut self.dirty_structure);
        let moved = std::mem::take(&mut self.dirty_motion);
        if !structural && !moved {
            return Commit::Unchanged;
        }
        let boxes = self.boxes();
        let rebuilt = structural || !self.tlas.refit(&boxes) || self.tlas.needs_rebuild();
        if rebuilt {
            self.tlas = Tlas::build(&boxes);
        }
        if structural {
            self.structure += 1;
        }
        self.motion += 1;
        if rebuilt {
            Commit::Rebuilt
        } else {
            Commit::Refit
        }
    }

    fn boxes(&self) -> Vec<Aabb> {
        self.instances
            .iter()
            .map(|s| s.as_ref().map_or(Aabb::EMPTY, |s| s.bounds))
            .collect()
    }

    fn acquire_mesh(&mut self, mesh: MeshHash) {
        if let Some(m) = self.meshes.get_mut(&mesh) {
            m.users += 1;
        }
    }

    fn release_mesh(&mut self, mesh: MeshHash) {
        if let Some(m) = self.meshes.get_mut(&mesh) {
            m.users = m.users.saturating_sub(1);
        }
    }

    fn bury(&mut self, id: u64, key: InstanceKey, mesh: MeshHash, death: Death) {
        self.epoch += 1;
        self.tombstones.push(Tombstone {
            epoch: self.epoch,
            id,
            key,
            mesh,
            death,
        });
    }

    /// The current key of instance `id`.
    pub(crate) fn instance_key(&self, id: u64) -> Option<InstanceKey> {
        self.by_id
            .get(&id)
            .and_then(|&s| self.instances[s as usize].as_ref())
            .map(|s| s.key)
    }

    /// Whether `key` is still live.
    pub(crate) fn is_live(&self, key: InstanceKey) -> bool {
        self.slot(key.slot).is_some_and(|s| s.key == key)
    }

    /// The stable primitive behind a cached `(key, triangle)` pair, or `None` once
    /// the instance died or the triangle is out of range.
    pub(crate) fn primitive(&self, key: InstanceKey, triangle: u32) -> Option<PrimitiveKey> {
        let s = self.slot(key.slot).filter(|s| s.key == key)?;
        let blas = &self.meshes[&s.instance.mesh].mesh.blas;
        (triangle < blas.stats.triangles).then(|| PrimitiveKey {
            mesh: s.instance.mesh,
            submesh: blas.submesh_of(triangle),
            triangle,
        })
    }

    /// Instance slot `slot`, if live.
    pub(crate) fn slot(&self, slot: u32) -> Option<&InstanceSlot> {
        self.instances.get(slot as usize).and_then(Option::as_ref)
    }

    /// Tombstones issued after `epoch` (pass the last [`Self::epoch`] you read).
    pub(crate) fn tombstones_since(&self, epoch: u64) -> &[Tombstone] {
        let first = self.tombstones.partition_point(|t| t.epoch <= epoch);
        &self.tombstones[first..]
    }

    /// Drops tombstones at or before `epoch` once every reader has seen them.
    pub(crate) fn forget_tombstones(&mut self, epoch: u64) {
        let keep = self.tombstones.partition_point(|t| t.epoch <= epoch);
        self.tombstones.drain(..keep);
    }

    /// The newest tombstone's epoch.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Versions `TraceGpu` compares to decide what to upload: (structure, motion,
    /// materials).
    pub(crate) fn versions(&self) -> (u64, u64, u64) {
        (self.structure, self.motion, self.materials)
    }

    pub(crate) fn tlas(&self) -> &Tlas {
        &self.tlas
    }

    pub(crate) fn instance_slots(&self) -> &[Option<InstanceSlot>] {
        &self.instances
    }

    /// Live meshes in BLAS slot order.
    pub(crate) fn blas_slots(&self) -> impl Iterator<Item = (u32, &TraceMesh)> {
        self.blas_slots
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.as_ref().map(|h| (i as u32, &self.meshes[h].mesh)))
    }

    pub(crate) fn blas_slot_count(&self) -> usize {
        self.blas_slots.len()
    }

    /// BLAS key of a registered mesh.
    pub(crate) fn blas_key(&self, mesh: MeshHash) -> Option<BlasKey> {
        self.meshes.get(&mesh).map(|m| m.key)
    }

    pub(crate) fn alpha_materials(&self) -> &[Option<AlphaMaterial>] {
        &self.alpha
    }

    /// Whether an instance's materials include an alpha test.
    pub(crate) fn has_alpha(&self, instance: &TraceInstance) -> bool {
        instance
            .materials
            .iter()
            .any(|&m| m != NO_MATERIAL && self.alpha.get(m as usize).is_some_and(Option::is_some))
    }
}
