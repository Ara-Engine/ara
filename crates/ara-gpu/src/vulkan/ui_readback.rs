//! Picking reads a retained completed ID image on the UI queue, never Scene work.
use super::*;

impl VulkanDevice {
    /// Read one R32F ID texel on the native UI queue. Only this tiny copy waits;
    /// an unrelated Scene submission may remain in flight on the other queue.
    ///
    /// # Safety
    /// `image` must be immutable, completed, published to the UI queue, and in
    /// CopySource layout. It must stay so through this call. UI use of this device
    /// and queue must be serialized with this owner, as for native presentation.
    pub unsafe fn read_ui_pixel(&mut self, image: Image, pixel: [u32; 2]) -> Result<[u8; 4]> {
        let image = self.images.get(image)?.raw;
        if image.format != vk::Format::R32_SFLOAT
            || pixel[0] >= image.width
            || pixel[1] >= image.height
            || !image.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
        {
            return Err(GpuError::Contract("Invalid presented ID pixel".into()));
        }
        let buffer = self
            .context
            .buffer(4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let result = (|| -> anyhow::Result<[u8; 4]> {
            let context = &self.context;
            let cmd = context.commands(false)?;
            let mut guard = guard::Objects::new(&context.device);
            guard.push(guard::Object::Commands(context.graphics_pool, cmd));
            // SAFETY: The source is immutable and published by the caller. The
            // destination and command are fresh. Fence completion precedes host
            // access and guard destruction; no Scene queue wait is requested.
            unsafe {
                context.begin(cmd)?;
                context.device.cmd_copy_image_to_buffer(
                    cmd,
                    image.handle,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer.handle,
                    &[vk::BufferImageCopy::default()
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .layer_count(1),
                        )
                        .image_offset(vk::Offset3D {
                            x: pixel[0] as i32,
                            y: pixel[1] as i32,
                            z: 0,
                        })
                        .image_extent(vk::Extent3D {
                            width: 1,
                            height: 1,
                            depth: 1,
                        })],
                );
                context.device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[
                        vk::BufferMemoryBarrier2::default()
                            .buffer(buffer.handle)
                            .size(4)
                            .src_stage_mask(vk::PipelineStageFlags2::COPY)
                            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                            .dst_access_mask(vk::AccessFlags2::HOST_READ)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED),
                    ]),
                );
                context.device.end_command_buffer(cmd)?;
                let fence = context
                    .device
                    .create_fence(&vk::FenceCreateInfo::default(), None)?;
                guard.push(guard::Object::Fence(fence));
                context.device.queue_submit2(
                    context.graphics,
                    &[vk::SubmitInfo2::default().command_buffer_infos(&[
                        vk::CommandBufferSubmitInfo::default().command_buffer(cmd),
                    ])],
                    fence,
                )?;
                context.measure_host_wait(|| {
                    context.device.wait_for_fences(&[fence], true, u64::MAX)
                })?;
            }
            let bytes = context.read(buffer)?;
            bytes.as_slice().try_into().map_err(anyhow::Error::from)
        })();
        self.context.destroy_buffer(buffer);
        result.map_err(Into::into)
    }
}
