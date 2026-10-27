//! Shared native geometry allocations; instance transforms live in separate frame data.
use super::{Scene, Vertex};
use anyhow::{Result, ensure};
use ara_gpu::{
    AccelerationStructure, Access, Buffer, Device, GpuDevice, Resource, acceleration::TriangleRange,
};

mod pack;
mod pose;
pub(crate) mod preparation;
pub use pose::PreparedPose;
#[cfg(test)]
mod tests;
mod upload;

/// Contiguous entries for one geometry's cooked LODs in the native GPU table.
#[derive(Debug, Clone, Copy)]
pub struct LodRange {
    /// Finest-LOD table entry.
    pub first: u32,
    /// Number of finest-to-coarsest entries.
    pub count: u32,
}

/// Content upload measurements; native AS backing/scratch is outside buffer payloads.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct GeometryUpload {
    /// Unique source geometries, independent of instance count.
    pub geometries: usize,
    /// Shared vertex/index and geometry-table payload, including three skin-pose slots.
    pub bytes: u64,
    /// Bytes written from the host for this generation, including skin slots/table.
    pub uploaded_bytes: u64,
    /// Unchanged static vertex/index bytes copied directly from the previous pool.
    pub reused_bytes: u64,
    /// Source geometries whose static data was copied from the resident generation.
    pub reused_geometries: usize,
    /// Native BLAS objects, one per unique geometry LOD when RT is requested.
    pub blas: usize,
    /// GPU staging transfer duration.
    pub upload_ms: f64,
    /// GPU duration of the batched BLAS builds.
    pub build_ms: f64,
}

// Two uint4 rows: current vertex buffer/offset, index buffer/offset; previous
// vertex buffer/offset, vertex count, index count. Offsets are bytes, indices local.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Row([u32; 8]);

const TABLE_HOST_WRITE: u8 = 1 << 3;

/// Immutable model-space vertex/index allocation and one BLAS per cooked LOD.
/// Skin vertices have separate presented, published and candidate buffers; moving instances never copy the
/// static pool. The owner retires dependent TLAS objects before destroying this set.
pub struct GeometryGpu {
    static_pool: Option<Buffer>,
    dynamic: Option<[Buffer; 3]>,
    table: Option<(Buffer, u32)>,
    ranges: Vec<LodRange>,
    structures: Vec<AccelerationStructure>,
    layout: Vec<GeometryLayout>,
    lod_data: Vec<pack::PackedLod>,
    staging: Vec<Vertex>,
    current_slot: usize,
    presented_slot: usize,
    pending_pose: bool,
    // Bits 0..2 (skin slots) and bit 3 (row table) written since the consuming frame.
    pose_host_writes: u8,
    /// Vertex/index buffers are acceleration-structure build inputs (RT devices):
    /// callers may build their own BLASes over `triangles` (hybrid ray queries).
    ray_inputs: bool,
    stats: GeometryUpload,
}

#[derive(Clone)]
struct GeometryLayout {
    key: [u8; 32],
    revision: u64,
    deforming: bool,
    first: usize,
    vertices: usize,
}

impl GeometryGpu {
    /// Bindless geometry-row table; absent for a sky-only scene.
    pub fn table_index(&self) -> Option<u32> {
        self.table.map(|(_, index)| index)
    }

    /// LOD entries for one source geometry, shared by every instance of it.
    pub fn lods(&self, geometry: usize) -> Option<LodRange> {
        self.ranges.get(geometry).copied()
    }

    /// Triangle-list draw size for a geometry-table LOD entry.
    pub fn index_count(&self, entry: u32) -> Option<u32> {
        self.lod_data.get(entry as usize).map(|lod| lod.index_count)
    }

    /// First index (u32 elements into [`Self::index_buffer`]) of a LOD entry;
    /// its indices are local to the LOD's vertices.
    pub fn first_index(&self, entry: u32) -> Option<u32> {
        self.lod_data
            .get(entry as usize)
            .and_then(|lod| u32::try_from(lod.index_offset / 4).ok())
    }

    /// The static pool, which indexed draws bind as their index buffer; absent
    /// for a sky-only scene. It rests in `Access::Geometry` ([`Self::resources`]).
    pub fn index_buffer(&self) -> Option<Buffer> {
        self.static_pool
    }

    /// BLAS for a geometry-table entry; absent on the raster path.
    pub fn blas(&self, entry: u32) -> Option<AccelerationStructure> {
        self.structures.get(entry as usize).copied()
    }

    /// Triangle input of a geometry-table entry for a caller-owned BLAS (the raster
    /// renderer's hybrid ray queries): the current pose's vertices for deforming
    /// geometry, so a committed pose changes `vertices`. `None` when the buffers are
    /// not acceleration-structure inputs (devices without RT, seedless preparations).
    pub fn triangles(&self, entry: u32) -> Option<TriangleRange> {
        let lod = self
            .lod_data
            .get(entry as usize)
            .filter(|_| self.ray_inputs)?;
        let indices = self.static_pool?;
        let vertices = if lod.deforming {
            self.dynamic?[self.current_slot]
        } else {
            indices
        };
        Some(TriangleRange {
            vertices,
            vertex_offset: lod.vertex_offset,
            vertex_count: lod.vertex_count,
            vertex_stride: std::mem::size_of::<Vertex>() as u64,
            indices,
            index_offset: lod.index_offset,
            index_count: lod.index_count,
        })
    }

    /// Last content upload's byte counts and GPU timings.
    pub fn stats(&self) -> GeometryUpload {
        self.stats
    }

    /// Vertex/index/table resources and their initial access for the consuming graph;
    /// the static pool rests in `Access::Geometry` (index and shader reads).
    pub fn resources(&self) -> impl Iterator<Item = (Resource, Access)> + '_ {
        self.static_pool
            .into_iter()
            .map(|buffer| (Resource::Buffer(buffer), Access::Geometry))
            .chain(
                self.dynamic
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(slot, buffer)| {
                        (
                            Resource::Buffer(buffer),
                            if self.pose_host_writes & (1 << slot) != 0 {
                                Access::HostWrite
                            } else {
                                Access::ShaderRead
                            },
                        )
                    }),
            )
            .chain(self.table.map(|(buffer, _)| {
                (
                    Resource::Buffer(buffer),
                    if self.pose_host_writes & TABLE_HOST_WRITE != 0 {
                        Access::HostWrite
                    } else {
                        Access::ShaderRead
                    },
                )
            }))
    }

    /// Retires content after every dependent TLAS has been retired.
    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for structure in self.structures {
            gpu.retire(Resource::AccelerationStructure(structure))?;
        }
        for buffer in self
            .static_pool
            .into_iter()
            .chain(self.dynamic.into_iter().flatten())
            .chain(self.table.map(|(buffer, _)| buffer))
        {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        gpu.collect()?;
        Ok(())
    }
}
