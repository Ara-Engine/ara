//! Range validation is shared by legacy whole-buffer and native pooled BLAS inputs.
use super::*;

impl VulkanDevice {
    pub(super) fn triangle_input(&self, input: TriangleRange) -> Result<Geometry> {
        let vertices = self.buffers.get(input.vertices)?.raw;
        let indices = self.buffers.get(input.indices)?.raw;
        input.validate(
            (vertices.size, vertices.usage),
            (indices.size, indices.usage),
        )?;
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
