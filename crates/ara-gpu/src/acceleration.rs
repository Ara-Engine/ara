//! Static acceleration structure descriptions, owned through generational handles.
use crate::{AccelerationStructure, Buffer};

/// Triangle or instance input for a one-time acceleration structure build.
pub enum AccelerationDesc {
    /// One opaque triangle geometry; positions are the first float3 in each vertex.
    Triangles {
        /// Address-enabled AS build input vertex buffer.
        vertices: Buffer,
        /// Vertex count.
        vertex_count: u32,
        /// Vertex stride in bytes.
        vertex_stride: u64,
        /// Address-enabled AS build input U32 index buffer.
        indices: Buffer,
        /// Index count, divisible by three.
        index_count: u32,
    },
    /// One triangle geometry inside shared vertex/index allocations. Indices are
    /// local to the selected vertex range, enabling batched uploads and real instancing.
    TriangleRange(TriangleRange),
    /// A top-level structure over already allocated BLAS identities.
    Instances(Vec<AccelerationInstance>),
}
/// Checked byte ranges for one BLAS, without duplicating a model's GPU buffers.
#[derive(Clone, Copy)]
pub struct TriangleRange {
    /// Address-enabled AS build input vertex allocation.
    pub vertices: Buffer,
    /// Four-byte-aligned offset of the first vertex position.
    pub vertex_offset: u64,
    /// Number of vertices in this range.
    pub vertex_count: u32,
    /// Complete vertex-record stride in bytes; at least twelve, divisible by four.
    pub vertex_stride: u64,
    /// Address-enabled AS build input U32 index allocation; may equal `vertices`.
    pub indices: Buffer,
    /// Four-byte-aligned offset of the first local triangle index.
    pub index_offset: u64,
    /// Number of local indices, divisible by three.
    pub index_count: u32,
}
impl TriangleRange {
    /// Check the range against its buffers' `(size, usage)`: aligned, in bounds and
    /// allocated as address-enabled AS build input. Both backends share these rules.
    pub(crate) fn validate(
        &self,
        vertices: (u64, ash::vk::BufferUsageFlags),
        indices: (u64, ash::vk::BufferUsageFlags),
    ) -> crate::Result<()> {
        use ash::vk::BufferUsageFlags as U;
        let usage = U::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR | U::SHADER_DEVICE_ADDRESS;
        let end = |count: u32, stride: u64, offset: u64| {
            u64::from(count)
                .checked_mul(stride)
                .and_then(|bytes| bytes.checked_add(offset))
        };
        if self.vertex_count == 0
            || self.index_count == 0
            || !self.index_count.is_multiple_of(3)
            || self.vertex_stride < 12
            || !self.vertex_stride.is_multiple_of(4)
            || !self.vertex_offset.is_multiple_of(4)
            || !self.index_offset.is_multiple_of(4)
            || !vertices.1.contains(usage)
            || !indices.1.contains(usage)
            || end(self.vertex_count, self.vertex_stride, self.vertex_offset)
                .is_none_or(|end| end > vertices.0)
            || end(self.index_count, 4, self.index_offset).is_none_or(|end| end > indices.0)
        {
            return Err(crate::GpuError::Contract(
                "Invalid triangle AS input range".into(),
            ));
        }
        Ok(())
    }
}
/// One top-level instance. Referenced BLAS storage stays alive through TLAS use.
pub struct AccelerationInstance {
    /// Bottom-level geometry identity.
    pub geometry: AccelerationStructure,
    /// Row-major 3x4 object-to-world affine matrix.
    pub transform: [f32; 12],
    /// Application instance identity, limited to 24 bits by the native ABI.
    pub custom_index: u32,
    /// Visibility mask.
    pub mask: u8,
}
