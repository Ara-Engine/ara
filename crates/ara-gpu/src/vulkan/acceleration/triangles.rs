//! Range validation is shared by legacy whole-buffer and native pooled BLAS inputs.
use super::*;

impl VulkanDevice {
    pub(super) fn triangle_input(&self, input: TriangleRange) -> Result<Geometry> {
        let vertices = self.buffers.get(input.vertices)?.raw;
        let indices = self.buffers.get(input.indices)?.raw;
        let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
        if input.vertex_count == 0
            || input.index_count == 0
            || !input.index_count.is_multiple_of(3)
            || input.vertex_stride < 12
            || !input.vertex_stride.is_multiple_of(4)
            || !input.vertex_offset.is_multiple_of(4)
            || !input.index_offset.is_multiple_of(4)
            || !vertices.usage.contains(usage)
            || !indices.usage.contains(usage)
            || u64::from(input.vertex_count)
                .checked_mul(input.vertex_stride)
                .and_then(|bytes| bytes.checked_add(input.vertex_offset))
                .is_none_or(|end| end > vertices.size)
            || u64::from(input.index_count)
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(input.index_offset))
                .is_none_or(|end| end > indices.size)
        {
            return Err(GpuError::Contract("Invalid triangle AS input range".into()));
        }
        Ok(Geometry::Triangles {
            vertices: input.vertices,
            vertex_offset: input.vertex_offset,
            vertex_count: input.vertex_count,
            stride: input.vertex_stride,
            indices: input.indices,
            index_offset: input.index_offset,
            index_count: input.index_count,
        })
    }

    pub(super) fn range_address(&self, buffer: Buffer, offset: u64) -> Result<u64> {
        self.context
            .address(self.buffers.get(buffer)?.raw)
            .checked_add(offset)
            .ok_or_else(|| GpuError::Contract("Triangle AS input address overflow".into()))
    }
}
