//! Buffer and texture transfer encoding.
use super::*;
impl MetalDevice {
    /// GPU address of an indirect argument record in an `INDIRECT_BUFFER` buffer.
    pub(super) fn indirect_address(
        &self,
        buffer: crate::Buffer,
        offset: u64,
        len: u64,
    ) -> Result<u64> {
        let raw = &self.buffers.get(buffer)?.raw;
        if !offset.is_multiple_of(4)
            || offset.checked_add(len).is_none_or(|end| end > raw.size)
            || !raw.usage.contains(vk::BufferUsageFlags::INDIRECT_BUFFER)
        {
            return Err(GpuError::Contract("Indirect record exceeds buffer".into()));
        }
        Ok(raw.buffer.gpuAddress() + offset)
    }

    pub(super) fn copy_buffer(
        &self,
        rec: &mut Recording,
        source: crate::Buffer,
        source_offset: u64,
        destination: crate::Buffer,
        destination_offset: u64,
        size: u64,
    ) -> Result<()> {
        let (from, to) = (self.buffers.get(source)?, self.buffers.get(destination)?);
        let fits =
            |offset: u64, total: u64| offset.checked_add(size).is_some_and(|end| end <= total);
        if source == destination
            || !from.raw.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
            || !to.raw.usage.contains(vk::BufferUsageFlags::TRANSFER_DST)
            || size == 0
            || !size.is_multiple_of(4)
            || !source_offset.is_multiple_of(4)
            || !destination_offset.is_multiple_of(4)
            || !fits(source_offset, from.raw.size)
            || !fits(destination_offset, to.raw.size)
        {
            return Err(GpuError::Contract("Invalid buffer copy".into()));
        }
        let (from, to) = (from.raw.buffer.clone(), to.raw.buffer.clone());
        // SAFETY: both ranges were bounds-checked; the buffers are distinct and resident.
        unsafe {
            rec.compute(self)?
                .copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
                    &from,
                    source_offset as usize,
                    &to,
                    destination_offset as usize,
                    size as usize,
                );
        }
        Ok(())
    }

    /// Tightly packed buffer <-> image copies of every level, finest first.
    pub(super) fn copy_image_buffer(
        &self,
        rec: &mut Recording,
        buffer: crate::Buffer,
        image: crate::Image,
        upload: bool,
    ) -> Result<()> {
        let raw = &self.images.get(image)?.raw;
        let mut offset = 0u64;
        for mip in 0..raw.levels {
            offset = offset.next_multiple_of(if raw.info.depth {
                4
            } else {
                u64::from(raw.info.block_bytes)
            });
            let region = crate::texture::ImageRegion {
                mip,
                origin: [0; 3],
                extent: raw.shape.extent(raw.size, mip),
                base_layer: 0,
                layer_count: raw.shape.layers,
            };
            let (_, bytes) = raw.shape.region(
                raw.size,
                region,
                raw.info.block,
                u64::from(raw.info.block_bytes),
            )?;
            self.copy_image_region_buffer(rec, buffer, image, offset, region, upload)?;
            offset += bytes;
        }
        Ok(())
    }

    pub(super) fn copy_image_region_buffer(
        &self,
        rec: &mut Recording,
        buffer: crate::Buffer,
        image: crate::Image,
        offset: u64,
        region: crate::texture::ImageRegion,
        upload: bool,
    ) -> Result<()> {
        let b = &self.buffers.get(buffer)?.raw;
        let i = &self.images.get(image)?.raw;
        let (row, total) =
            i.shape
                .region(i.size, region, i.info.block, u64::from(i.info.block_bytes))?;
        if (i.info.depth && !offset.is_multiple_of(4))
            || !offset.is_multiple_of(u64::from(i.info.block_bytes))
            || offset.checked_add(total).is_none_or(|end| end > b.size)
            || !b.usage.contains(if upload {
                vk::BufferUsageFlags::TRANSFER_SRC
            } else {
                vk::BufferUsageFlags::TRANSFER_DST
            })
            || !i.usage.contains(if upload {
                vk::ImageUsageFlags::TRANSFER_DST
            } else {
                vk::ImageUsageFlags::TRANSFER_SRC
            })
        {
            return Err(GpuError::Contract(
                "Invalid buffer/texture copy usage, offset or capacity".into(),
            ));
        }
        let plane = row * u64::from(region.extent[1].div_ceil(i.info.block));
        let per_layer = plane * u64::from(region.extent[2]);
        let option = if i.info.depth {
            MTLBlitOption::DepthFromDepthStencil
        } else {
            MTLBlitOption::None
        };
        let encoder = rec.compute(self)?;
        for layer in 0..region.layer_count {
            let at = offset + u64::from(layer) * per_layer;
            // SAFETY: validated footprint and native subresources; buffers and textures stay resident.
            unsafe {
                if upload {
                    encoder.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin_options(
                        &b.buffer,at as usize,row as usize,plane as usize,extent(region.extent),&i.texture,(region.base_layer+layer) as usize,region.mip as usize,origin(region.origin),option);
                } else {
                    encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage_options(
                        &i.texture,(region.base_layer+layer) as usize,region.mip as usize,origin(region.origin),extent(region.extent),&b.buffer,at as usize,row as usize,plane as usize,option);
                }
            }
        }
        Ok(())
    }
}
