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
    /// A top-level structure over already allocated BLAS identities.
    Instances(Vec<AccelerationInstance>),
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
