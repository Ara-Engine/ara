//! Triangle positions fetched from retained mesh buffers, without an AS position-return intrinsic.
use crate::{BindlessHeap, Device, GpuError, Result, acceleration::TriangleRange};
/// Shared Slang/MSL helper source. Slang defaults to a global `buffers` heap. MSL
/// adapters define ARA_HIT_CONTEXT, ARA_HIT_LOAD and ARA_HIT_FLOAT3 before inclusion.
/// The helper returns object-space positions in index/barycentric order and a validity word.
/// `tableOffset` must be four-byte aligned and the table must contain `instanceCount`
/// complete records; mesh range checks do not validate the caller-owned table.
pub const SHADER: &str = include_str!("../shaders/hit_geometry.slang");
/// A 32-byte shader record describing one indexed position stream. Store records
/// in TLAS instance-index order, with the same ranges used to build those instances'
/// BLASes. Retain and declare both mesh buffers and this record table in every query pass.
/// Refits use current mesh positions; compaction does not change the record.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HitGeometry {
    words: [u32; 8],
}
impl HitGeometry {
    /// Publish shader-readable mesh buffers and encode an already AS-validated range.
    /// Buffers must also declare storage usage. Byte-address loads address at most
    /// 4 GiB; unsupported ranges fail before publication. Instance indices are native
    /// TLAS ordinals (`CommittedInstanceIndex`), not application custom IDs.
    pub fn publish(gpu: &mut impl Device, range: TriangleRange) -> Result<Self> {
        let fits = |offset: u64, count: u32, stride: u64| {
            offset
                .checked_add(u64::from(count).checked_mul(stride)?)
                .filter(|end| *end <= u64::from(u32::MAX))
        };
        if range.vertex_count == 0
            || range.index_count == 0
            || !range.index_count.is_multiple_of(3)
            || range.vertex_stride < 12
            || !range.vertex_stride.is_multiple_of(4)
            || !range.vertex_offset.is_multiple_of(4)
            || !range.index_offset.is_multiple_of(4)
            || fits(range.vertex_offset, range.vertex_count, range.vertex_stride).is_none()
            || fits(range.index_offset, range.index_count, 4).is_none()
        {
            return Err(GpuError::Contract(
                "Hit geometry range exceeds aligned byte-address records".into(),
            ));
        }
        let vertices = gpu.heap().insert_buffer(range.vertices)?;
        let indices = gpu.heap().insert_buffer(range.indices)?;
        Ok(Self {
            words: [
                vertices.index,
                indices.index,
                range.vertex_offset as u32,
                range.index_offset as u32,
                range.vertex_stride as u32,
                range.vertex_count,
                range.index_count,
                0,
            ],
        })
    }
}
