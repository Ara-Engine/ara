//! GPU storage of a [`TraceScene`]: eight storage buffers in the shared bindless
//! heap, named by one 32-byte scene record (`shaders/trace.slangh` `TraceScene`) that
//! callers pass to `traceScene(record)`.
//!
//! - BLAS nodes, compressed triangles and alpha UV tables of every mesh are
//!   device-local and uploaded once per structure (meshes or instance slots added or
//!   removed): [`TraceGpu::update`] then republishes.
//! - The TLAS, instance records, per-instance material indices and the alpha table
//!   are host-coherent and rewritten in place (`upload_queued_range`) on moves and
//!   material edits, so refits allocate nothing.
use super::scene::{NO_MATERIAL, TraceScene};
use anyhow::Result;
use ara_gpu::vulkan::Command;
use ara_gpu::{Access, Barrier, Buffer, Device, GpuDevice, Queue, Resource, vk};
use bytemuck::{Pod, Zeroable};

/// Bytes of one instance record (`TraceInstanceRecord`).
pub(crate) const INSTANCE_BYTES: usize = 80;
/// Bytes of one alpha material record (`TraceAlphaMaterial`).
pub(crate) const ALPHA_BYTES: usize = 48;
/// Bytes of one source triangle's alpha UVs (both sets, three corners).
pub(crate) const UV_BYTES: usize = 48;
/// Instance flags, in bits 8-15 of record word `a.z` (bits 0-7 hold the mask).
pub(crate) const INSTANCE_MIRRORED: u32 = 1 << 8;
/// The instance has an alpha-tested material.
pub(crate) const INSTANCE_ALPHA: u32 = 1 << 9;
/// Scene flags: some instance exists.
pub(crate) const SCENE_NONEMPTY: u32 = 1;
/// Scene flags: some instance is alpha tested.
pub(crate) const SCENE_ALPHA: u32 = 2;

/// The scene record: heap indices of the eight buffers and the scene flags.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub(crate) struct SceneRecord {
    pub nodes: u32,
    pub triangles: u32,
    pub tlas: u32,
    pub instances: u32,
    pub materials: u32,
    pub alpha: u32,
    pub uvs: u32,
    pub flags: u32,
}

/// What [`TraceGpu::update`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Upload {
    /// Nothing changed since the last upload.
    Unchanged,
    /// Dynamic buffers were rewritten in place.
    Rewritten,
    /// The structure changed: every buffer was allocated again (the old ones
    /// retire after their last GPU use; the record index changed).
    Republished,
}

struct Static {
    nodes: Buffer,
    triangles: Buffer,
    uvs: Buffer,
    /// Per BLAS slot: node base, triangle word base, UV row base (or MAX).
    bases: Vec<[u32; 3]>,
}

struct Dynamic {
    tlas: Buffer,
    instances: Buffer,
    materials: Buffer,
    alpha: Buffer,
    /// Byte capacity of the alpha table (it may grow without a structure change).
    alpha_capacity: usize,
    record: Buffer,
}

/// The uploaded scene. Owners declare [`Self::buffers`] in each graph that runs
/// `trace()` and pass [`Self::record`] to shaders.
pub(crate) struct TraceGpu {
    statics: Static,
    dynamic: Dynamic,
    indices: SceneRecord,
    record_index: u32,
    versions: (u64, u64, u64),
}

/// Device-local storage holding `bytes` (at least 16), filled through a staging copy.
pub(super) fn upload_static(gpu: &mut GpuDevice, bytes: &[u8]) -> Result<Buffer> {
    let size = bytes.len().max(16) as u64;
    let staging = gpu.create_buffer(size, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let mut padded = bytes.to_vec();
    padded.resize(size as usize, 0);
    let result = (|| {
        gpu.upload(staging, &padded)?;
        let buffer = gpu.create_buffer(
            size,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            false,
        )?;
        let mut recorder = gpu.begin(Queue::Graphics);
        recorder.push(Command::CopyBuffer {
            source: staging,
            destination: buffer,
            size,
        });
        recorder.barriers([Barrier {
            resource: Resource::Buffer(buffer),
            before: Access::CopyDestination,
            after: Access::ShaderRead,
            handoff: None,
        }]);
        let point = gpu.submit(recorder.finish(), &[])?;
        gpu.resolve(point)?;
        Ok(buffer)
    })();
    gpu.retire(Resource::Buffer(staging))?;
    result
}

/// Host-coherent storage of `capacity` bytes (at least 16) holding `bytes`.
fn host_buffer(gpu: &mut GpuDevice, bytes: &[u8], capacity: usize) -> Result<Buffer> {
    let size = capacity.max(bytes.len()).max(16) as u64;
    let buffer = gpu.create_buffer(size, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    if !bytes.is_empty() {
        gpu.upload(buffer, bytes)?;
    }
    Ok(buffer)
}

/// The packed BLAS pool.
struct Pool {
    nodes: Vec<[u32; 4]>,
    tris: Vec<[u32; 4]>,
    uvs: Vec<[f32; 12]>,
    /// Per BLAS slot: node base, triangle word base, UV row base (or MAX).
    bases: Vec<[u32; 3]>,
}

/// The BLAS pool: nodes, triangles and UV rows of every live mesh, in slot order.
fn pack_static(scene: &TraceScene) -> Pool {
    let (mut nodes, mut tris, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
    let mut bases = vec![[u32::MAX; 3]; scene.blas_slot_count()];
    for (slot, mesh) in scene.blas_slots() {
        let uv_base = match &mesh.uvs {
            Some(rows) => {
                let base = uvs.len() as u32;
                uvs.extend_from_slice(rows);
                base
            }
            None => u32::MAX,
        };
        bases[slot as usize] = [
            (nodes.len() / ara_bvh::NODE_WORDS) as u32,
            tris.len() as u32,
            uv_base,
        ];
        nodes.extend_from_slice(&mesh.blas.nodes);
        tris.extend_from_slice(&mesh.blas.triangles);
    }
    Pool {
        nodes,
        tris,
        uvs,
        bases,
    }
}

/// Instance records, per-instance material indices and the scene flags.
fn pack_instances(scene: &TraceScene, bases: &[[u32; 3]]) -> (Vec<u8>, Vec<u32>, u32) {
    let slots = scene.instance_slots();
    let mut records = vec![0u8; slots.len().max(1) * INSTANCE_BYTES];
    let mut materials = Vec::new();
    let mut flags = if scene.tlas().nodes.is_empty() {
        0
    } else {
        SCENE_NONEMPTY
    };
    for (i, slot) in slots.iter().enumerate() {
        let Some(s) = slot else { continue };
        let Some(key) = scene.blas_key(s.instance.mesh) else {
            continue;
        };
        let [node_base, tri_base, uv_base] = bases[key.slot as usize];
        let world = s.instance.world;
        let det = world.matrix3.determinant();
        let (rows, mut mask) = if det != 0.0 && det.is_finite() {
            let inv = world.inverse();
            let (m, t) = (inv.matrix3, inv.translation);
            let row = |r: usize| [m.x_axis[r], m.y_axis[r], m.z_axis[r], t[r]];
            ([row(0), row(1), row(2)], u32::from(s.instance.mask))
        } else {
            // A singular transform flattens the instance: nothing can hit it.
            ([[0.0; 4]; 3], 0)
        };
        if det < 0.0 {
            mask |= INSTANCE_MIRRORED;
        }
        // Every instance owns its submeshes' entries, so the array's size depends on
        // the structure alone and material edits rewrite it in place.
        let material_base = materials.len() as u32;
        materials.extend_from_slice(&s.instance.materials);
        if scene.has_alpha(&s.instance) && uv_base != u32::MAX {
            mask |= INSTANCE_ALPHA;
            flags |= SCENE_ALPHA;
        }
        let generation = s.key.generation;
        let words: [u32; 20] = {
            let mut w = [0u32; 20];
            for (r, row) in rows.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    w[r * 4 + c] = v.to_bits();
                }
            }
            w[12..16].copy_from_slice(&[node_base, tri_base, mask, material_base]);
            w[16..20].copy_from_slice(&[
                s.instance.user,
                uv_base,
                generation as u32,
                (generation >> 32) as u32,
            ]);
            w
        };
        records[i * INSTANCE_BYTES..(i + 1) * INSTANCE_BYTES]
            .copy_from_slice(bytemuck::cast_slice(&words));
    }
    (records, materials, flags)
}

/// The alpha table: one record per material index (`texture` MAX: opaque).
fn pack_alpha(scene: &TraceScene) -> Vec<u8> {
    let mut out = Vec::with_capacity(scene.alpha_materials().len() * ALPHA_BYTES);
    for m in scene.alpha_materials() {
        let words: [u32; 12] = match m {
            Some(m) => [
                m.texture,
                m.uv_set,
                m.cutoff.to_bits(),
                m.factor.to_bits(),
                m.offset[0].to_bits(),
                m.offset[1].to_bits(),
                m.scale[0].to_bits(),
                m.scale[1].to_bits(),
                m.rotation.to_bits(),
                0,
                0,
                0,
            ],
            None => {
                let mut w = [0; 12];
                w[0] = NO_MATERIAL;
                w
            }
        };
        out.extend_from_slice(bytemuck::cast_slice(&words));
    }
    out
}

fn tlas_bytes(scene: &TraceScene) -> &[u8] {
    bytemuck::cast_slice(&scene.tlas().nodes)
}

impl TraceGpu {
    /// Uploads `scene` (after [`TraceScene::commit`]).
    pub(crate) fn new(gpu: &mut GpuDevice, scene: &TraceScene) -> Result<Self> {
        let Pool {
            nodes,
            tris,
            uvs,
            bases,
        } = pack_static(scene);
        let mut allocated: Vec<Buffer> = Vec::new();
        let result = (|| -> Result<Self> {
            let mut make_static = |gpu: &mut GpuDevice, bytes: &[u8]| -> Result<Buffer> {
                let b = upload_static(gpu, bytes)?;
                allocated.push(b);
                Ok(b)
            };
            let statics = Static {
                nodes: make_static(gpu, bytemuck::cast_slice(&nodes))?,
                triangles: make_static(gpu, bytemuck::cast_slice(&tris))?,
                uvs: make_static(gpu, bytemuck::cast_slice(&uvs))?,
                bases,
            };
            let (records, materials, flags) = pack_instances(scene, &statics.bases);
            let alpha = pack_alpha(scene);
            let alpha_capacity = (alpha.len().max(ALPHA_BYTES)).next_power_of_two();
            let mut make_host = |gpu: &mut GpuDevice, bytes: &[u8], cap: usize| -> Result<Buffer> {
                let b = host_buffer(gpu, bytes, cap)?;
                allocated.push(b);
                Ok(b)
            };
            let dynamic = Dynamic {
                tlas: make_host(gpu, tlas_bytes(scene), 0)?,
                instances: make_host(gpu, &records, 0)?,
                materials: make_host(gpu, bytemuck::cast_slice(&materials), 0)?,
                alpha: make_host(gpu, &alpha, alpha_capacity)?,
                alpha_capacity,
                record: make_host(gpu, &[0; 32], 32)?,
            };
            let index = |gpu: &mut GpuDevice, b: Buffer| -> Result<u32> {
                Ok(gpu.storage_buffer(b)?.index)
            };
            let indices = SceneRecord {
                nodes: index(gpu, statics.nodes)?,
                triangles: index(gpu, statics.triangles)?,
                tlas: index(gpu, dynamic.tlas)?,
                instances: index(gpu, dynamic.instances)?,
                materials: index(gpu, dynamic.materials)?,
                alpha: index(gpu, dynamic.alpha)?,
                uvs: index(gpu, statics.uvs)?,
                flags,
            };
            gpu.upload(dynamic.record, bytemuck::bytes_of(&indices))?;
            let record_index = index(gpu, dynamic.record)?;
            Ok(Self {
                statics,
                dynamic,
                indices,
                record_index,
                versions: scene.versions(),
            })
        })();
        if result.is_err() {
            for b in allocated {
                gpu.retire(Resource::Buffer(b))?;
            }
        }
        result
    }

    /// Brings the GPU copy up to date with `scene` (after [`TraceScene::commit`]).
    pub(crate) fn update(&mut self, gpu: &mut GpuDevice, scene: &TraceScene) -> Result<Upload> {
        let versions = scene.versions();
        if versions == self.versions {
            return Ok(Upload::Unchanged);
        }
        if versions.0 != self.versions.0 {
            let fresh = Self::new(gpu, scene)?;
            let old = std::mem::replace(self, fresh);
            old.destroy(gpu)?;
            return Ok(Upload::Republished);
        }
        let (records, materials, flags) = pack_instances(scene, &self.statics.bases);
        gpu.upload_queued_range(self.dynamic.tlas, 0, tlas_bytes(scene))?;
        gpu.upload_queued_range(self.dynamic.instances, 0, &records)?;
        if versions.2 != self.versions.2 {
            if !materials.is_empty() {
                gpu.upload_queued_range(
                    self.dynamic.materials,
                    0,
                    bytemuck::cast_slice(&materials),
                )?;
            }
            let alpha = pack_alpha(scene);
            if alpha.len() > self.dynamic.alpha_capacity {
                // A material index past the table: the one allocation an edit makes.
                let capacity = alpha.len().next_power_of_two();
                let fresh = host_buffer(gpu, &alpha, capacity)?;
                gpu.retire(Resource::Buffer(std::mem::replace(
                    &mut self.dynamic.alpha,
                    fresh,
                )))?;
                self.dynamic.alpha_capacity = capacity;
                self.indices.alpha = gpu.storage_buffer(fresh)?.index;
            } else if !alpha.is_empty() {
                gpu.upload_queued_range(self.dynamic.alpha, 0, &alpha)?;
            }
        }
        if flags != self.indices.flags || versions.2 != self.versions.2 {
            self.indices.flags = flags;
            gpu.upload_queued_range(self.dynamic.record, 0, bytemuck::bytes_of(&self.indices))?;
        }
        self.versions = versions;
        Ok(Upload::Rewritten)
    }

    /// Heap index of the scene record (`traceScene(record)`).
    pub(crate) fn record(&self) -> u32 {
        self.record_index
    }

    /// The record's contents (the eight heap indices).
    pub(crate) fn indices(&self) -> SceneRecord {
        self.indices
    }

    /// Every buffer shaders reach through the record, for graph declarations.
    pub(crate) fn buffers(&self) -> [Buffer; 8] {
        [
            self.statics.nodes,
            self.statics.triangles,
            self.statics.uvs,
            self.dynamic.tlas,
            self.dynamic.instances,
            self.dynamic.materials,
            self.dynamic.alpha,
            self.dynamic.record,
        ]
    }

    /// Device bytes of the BLAS pool (nodes and triangles).
    pub(crate) fn blas_bytes(&self, scene: &TraceScene) -> usize {
        scene.blas_slots().map(|(_, m)| m.blas.bytes()).sum()
    }

    /// Retires every buffer after its last GPU use.
    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for b in self.buffers() {
            gpu.retire(Resource::Buffer(b))?;
        }
        Ok(())
    }
}
