use super::*;
use resources::barrier;
const COPY: (vk::PipelineStageFlags2, vk::AccessFlags2) = (
    vk::PipelineStageFlags2::COPY,
    vk::AccessFlags2::TRANSFER_READ,
);
const SAMPLE: (vk::PipelineStageFlags2, vk::AccessFlags2) = (
    vk::PipelineStageFlags2::FRAGMENT_SHADER,
    vk::AccessFlags2::SHADER_SAMPLED_READ,
);
const COLOR: (vk::PipelineStageFlags2, vk::AccessFlags2) = (
    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
);
impl NativeUi {
    fn start(&self) -> Result<()> {
        // SAFETY: Every submit waits for this fence before returning; command buffer is idle here.
        unsafe {
            self.memory
                .device
                .reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())?;
            self.memory.device.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        let device = &self.memory.device;
        // SAFETY: This main thread serializes queue use; fence completion owns command-buffer reuse.
        unsafe {
            device.end_command_buffer(self.command)?;
            device.reset_fences(&[self.fence])?;
            device.queue_submit2(
                self.queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&[
                    vk::CommandBufferSubmitInfo::default().command_buffer(self.command),
                ])],
                self.fence,
            )?;
            device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }
        Ok(())
    }
    pub(super) fn upload_texture(&self, image: &Image, staging: &Buffer) -> Result<()> {
        self.start()?;
        let device = &self.memory.device;
        // SAFETY: New image has no prior contents; host-coherent staging was filled before submission.
        unsafe {
            device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier(
                    image.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                )]),
            );
            device.cmd_copy_buffer_to_image(
                self.command,
                staging.buffer,
                image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: image.size[0],
                        height: image.size[1],
                        depth: 1,
                    })],
            );
            device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier(
                    image.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                    SAMPLE,
                )]),
            );
        }
        self.finish()
    }
    pub(super) fn submit(&mut self) -> Result<()> {
        let instance_size = (self.instances.len().max(1) * 64) as u64;
        let mask_size = (self.masks.len().max(1) * 16) as u64;
        if self
            .instance_buffer
            .as_ref()
            .is_none_or(|b| b.size < instance_size)
        {
            self.instance_buffer = Some(self.memory.buffer(
                instance_size.next_power_of_two().max(65536),
                vk::BufferUsageFlags::STORAGE_BUFFER,
            )?);
        }
        if self.mask_buffer.as_ref().is_none_or(|b| b.size < mask_size) {
            self.mask_buffer = Some(self.memory.buffer(
                mask_size.next_power_of_two().max(16384),
                vk::BufferUsageFlags::STORAGE_BUFFER,
            )?);
        }
        let instances = self
            .instance_buffer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI instance buffer"))?;
        // SAFETY: Instance is repr(C), four initialized float4s, 64 bytes, with no internal/tail padding.
        let bytes = unsafe {
            std::slice::from_raw_parts(self.instances.as_ptr().cast(), self.instances.len() * 64)
        };
        instances.write(bytes)?;
        let masks = self
            .mask_buffer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI mask buffer"))?;
        if self.masks.is_empty() {
            masks.write(&[0; 16])?;
        } else {
            masks.write(bytemuck::cast_slice(&self.masks))?;
        }
        let instance_info = [vk::DescriptorBufferInfo::default()
            .buffer(instances.buffer)
            .range(instances.size)];
        let mask_info = [vk::DescriptorBufferInfo::default()
            .buffer(masks.buffer)
            .range(masks.size)];
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI target"))?;
        let device = &self.memory.device;
        // SAFETY: Previous submissions are complete; descriptors reference live coherent buffers.
        unsafe {
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&instance_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(3)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&mask_info),
                ],
                &[],
            );
        }
        self.start()?;
        let mut before = Vec::with_capacity(self.external_images.len() + 1);
        for image in self.external_images.values() {
            before.push(barrier(
                *image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                COPY,
                SAMPLE,
            ));
        }
        before.push(barrier(
            target.image,
            if self.target_ready {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            } else {
                vk::ImageLayout::UNDEFINED
            },
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            if self.target_ready {
                COPY
            } else {
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
            },
            COLOR,
        ));
        // SAFETY: All barriers describe the completed renderer and UI images' actual layouts.
        unsafe {
            device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().image_memory_barriers(&before),
            );
            let attachments = [vk::RenderingAttachmentInfo::default()
                .image_view(target.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { float32: [0.; 4] },
                })];
            device.cmd_begin_rendering(
                self.command,
                &vk::RenderingInfo::default()
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent: vk::Extent2D {
                            width: target.size[0],
                            height: target.size[1],
                        },
                    })
                    .layer_count(1)
                    .color_attachments(&attachments),
            );
            device.cmd_bind_pipeline(self.command, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                self.command,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.set],
                &[],
            );
            device.cmd_set_viewport(
                self.command,
                0,
                &[vk::Viewport {
                    x: 0.,
                    y: 0.,
                    width: target.size[0] as f32,
                    height: target.size[1] as f32,
                    min_depth: 0.,
                    max_depth: 1.,
                }],
            );
            for draw in &self.draws {
                if draw.scissor.extent.width == 0 || draw.scissor.extent.height == 0 {
                    continue;
                }
                device.cmd_set_scissor(self.command, 0, &[draw.scissor]);
                let words = [
                    draw.first,
                    draw.texture,
                    draw.mode,
                    draw.mask_offset,
                    (self.logical[0] as f32).to_bits(),
                    (self.logical[1] as f32).to_bits(),
                    draw.mask_count,
                    self.dpi.to_bits(),
                ];
                device.cmd_push_constants(
                    self.command,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytemuck::cast_slice(&words),
                );
                device.cmd_draw(self.command, 6, draw.count, 0, 0);
            }
            device.cmd_end_rendering(self.command);
            let after: Vec<_> = std::iter::once(barrier(
                target.image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                COLOR,
                COPY,
            ))
            .chain(self.external_images.values().map(|image| {
                barrier(
                    *image,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    SAMPLE,
                    COPY,
                )
            }))
            .collect();
            device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().image_memory_barriers(&after),
            );
        }
        self.finish()?;
        self.target_ready = true;
        Ok(())
    }
    pub(super) fn readback(&self) -> Result<image::RgbaImage> {
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI target not rendered"))?;
        let bytes = target.size[0] as u64 * target.size[1] as u64 * 4;
        let buffer = self
            .memory
            .buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST)?;
        self.start()?;
        // SAFETY: Target is in CopySource layout after UI end; buffer has enough coherent capacity.
        unsafe {
            let device = &self.memory.device;
            device.cmd_copy_image_to_buffer(
                self.command,
                target.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer.buffer,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: target.size[0],
                        height: target.size[1],
                        depth: 1,
                    })],
            );
            let barrier = vk::BufferMemoryBarrier2::default()
                .buffer(buffer.buffer)
                .offset(0)
                .size(bytes)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ);
            device.cmd_pipeline_barrier2(
                self.command,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier]),
            );
        }
        self.finish()?;
        image::RgbaImage::from_raw(target.size[0], target.size[1], buffer.read(bytes as usize)?)
            .ok_or_else(|| anyhow::anyhow!("UI screenshot dimensions"))
    }
}
