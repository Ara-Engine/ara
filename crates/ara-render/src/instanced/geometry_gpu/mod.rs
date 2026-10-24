//! Shared native geometry allocations; instance transforms live in separate frame data.
use super::{Scene, Vertex};
use anyhow::{Result, ensure};
use ara_gpu::{AccelerationStructure, Access, Buffer, Device, Resource, vulkan::VulkanDevice};

mod pack;
mod pose;
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
    stats: GeometryUpload,
}

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

    /// BLAS for a geometry-table entry; absent on the raster path.
    pub fn blas(&self, entry: u32) -> Option<AccelerationStructure> {
        self.structures.get(entry as usize).copied()
    }

    /// Last content upload's byte counts and GPU timings.
    pub fn stats(&self) -> GeometryUpload {
        self.stats
    }

    /// Vertex/index/table resources reached through the native shading ABI.
    pub fn resources(&self) -> impl Iterator<Item = (Resource, Access)> + '_ {
        self.static_pool
            .into_iter()
            .chain(self.dynamic.into_iter().flatten())
            .chain(self.table.map(|(buffer, _)| buffer))
            .map(|buffer| (Resource::Buffer(buffer), Access::ShaderRead))
    }

    /// Retires content after every dependent TLAS has been retired.
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
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
