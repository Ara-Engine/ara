use super::recording::access;
use super::*;
use crate::commands::*;

impl VulkanDevice {
    pub(super) fn encode(
        &self,
        cmd: vk::CommandBuffer,
        queue: Queue,
        query: vk::QueryPool,
        timestamp: &mut u32,
        command: &Command,
    ) -> Result<()> {
        let device = &self.context.device;
        // SAFETY: All handles were validated, scopes checked, and command buffer is recording.
        unsafe {
            match command {
                Command::Barrier(b) => {
                    let source_queue = b.handoff.map(|h| h.from).unwrap_or(queue);
                    let dest_queue = b.handoff.map(|h| h.to).unwrap_or(queue);
                    let (mut src_stage, mut src_access, old) = access(b.before, source_queue);
                    let (mut dst_stage, mut dst_access, new) = access(b.after, dest_queue);
                    let (from, to) = if let Some(h) = b.handoff {
                        if h.release {
                            dst_stage = vk::PipelineStageFlags2::NONE;
                            dst_access = vk::AccessFlags2::NONE;
                        } else {
                            src_stage = vk::PipelineStageFlags2::NONE;
                            src_access = vk::AccessFlags2::NONE;
                        }
                        (self.family(h.from), self.family(h.to))
                    } else {
                        (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
                    };
                    match b.resource {
                        Resource::AccelerationStructure(h) => {
                            let structure = self.accelerations.get(h)?;
                            let barrier = vk::BufferMemoryBarrier2::default()
                                .buffer(structure.raw.backing.handle)
                                .size(structure.raw.backing.size)
                                .src_stage_mask(src_stage)
                                .src_access_mask(src_access)
                                .dst_stage_mask(dst_stage)
                                .dst_access_mask(dst_access)
                                .src_queue_family_index(from)
                                .dst_queue_family_index(to);
                            device.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier]),
                            );
                        }
                        Resource::Image(h) => {
                            let i = self.images.get(h)?;
                            let barrier = image_barrier(
                                i.raw.handle,
                                i.raw.aspect,
                                old,
                                new,
                                (src_stage, src_access),
                                (dst_stage, dst_access),
                            )
                            .src_queue_family_index(from)
                            .dst_queue_family_index(to);
                            device.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
                            );
                        }
                        Resource::Buffer(h) => {
                            let b = self.buffers.get(h)?;
                            let barrier = vk::BufferMemoryBarrier2::default()
                                .buffer(b.raw.handle)
                                .size(b.raw.size)
                                .src_stage_mask(src_stage)
                                .src_access_mask(src_access)
                                .dst_stage_mask(dst_stage)
                                .dst_access_mask(dst_access)
                                .src_queue_family_index(from)
                                .dst_queue_family_index(to);
                            device.cmd_pipeline_barrier2(
                                cmd,
                                &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier]),
                            );
                        }
                    }
                }
                Command::Pipeline(h) => {
                    let p = self.pipelines.get(*h)?;
                    let bind = if p.compute {
                        vk::PipelineBindPoint::COMPUTE
                    } else {
                        vk::PipelineBindPoint::GRAPHICS
                    };
                    device.cmd_bind_pipeline(cmd, bind, p.handle);
                    device.cmd_bind_descriptor_sets(
                        cmd,
                        bind,
                        self.heap.layout,
                        0,
                        &[self.heap.set],
                        &[],
                    );
                }
                Command::Constants(c) => device.cmd_push_constants(
                    cmd,
                    self.heap.layout,
                    vk::ShaderStageFlags::ALL,
                    0,
                    bytemuck::bytes_of(c),
                ),
                Command::BuildAccelerationStructure(h) => self.encode_as_build(cmd, *h)?,
                Command::Dispatch(groups) => {
                    device.cmd_dispatch(cmd, groups[0], groups[1], groups[2])
                }
                Command::DispatchIndirect { arguments, offset } => {
                    device.cmd_dispatch_indirect(
                        cmd,
                        self.buffers.get(*arguments)?.raw.handle,
                        *offset,
                    );
                }
                Command::DrawIndirect { arguments, offset } => {
                    device.cmd_draw_indirect(
                        cmd,
                        self.buffers.get(*arguments)?.raw.handle,
                        *offset,
                        1,
                        16,
                    );
                }
                Command::BeginRendering {
                    colors,
                    depth,
                    size,
                } => {
                    let attachment = |a: &Attachment, depth: bool| -> Result<_> {
                        Ok(vk::RenderingAttachmentInfo::default()
                            .image_view(self.images.get(a.image)?.raw.view)
                            .image_layout(if depth {
                                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL
                            } else {
                                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                            })
                            .load_op(if a.clear.is_some() {
                                vk::AttachmentLoadOp::CLEAR
                            } else {
                                vk::AttachmentLoadOp::LOAD
                            })
                            .store_op(vk::AttachmentStoreOp::STORE)
                            .clear_value(if depth {
                                vk::ClearValue {
                                    depth_stencil: vk::ClearDepthStencilValue {
                                        depth: a.clear.unwrap_or([0.; 4])[0],
                                        stencil: 0,
                                    },
                                }
                            } else {
                                vk::ClearValue {
                                    color: vk::ClearColorValue {
                                        float32: a.clear.unwrap_or([0.; 4]),
                                    },
                                }
                            }))
                    };
                    let colors = colors
                        .iter()
                        .map(|a| attachment(a, false))
                        .collect::<Result<Vec<_>>>()?;
                    let depth = depth.as_ref().map(|a| attachment(a, true)).transpose()?;
                    let mut info = vk::RenderingInfo::default()
                        .render_area(vk::Rect2D {
                            offset: vk::Offset2D::default(),
                            extent: vk::Extent2D {
                                width: size[0],
                                height: size[1],
                            },
                        })
                        .layer_count(1)
                        .color_attachments(&colors);
                    if let Some(ref depth) = depth {
                        info = info.depth_attachment(depth);
                    }
                    device.cmd_begin_rendering(cmd, &info);
                    viewport(device, cmd, [0; 2], *size);
                }
                Command::Viewport { origin, size } => viewport(device, cmd, *origin, *size),
                Command::Draw(d) => {
                    device.cmd_bind_vertex_buffers(
                        cmd,
                        0,
                        &[self.buffers.get(d.vertices)?.raw.handle],
                        &[0],
                    );
                    device.cmd_bind_index_buffer(
                        cmd,
                        self.buffers.get(d.indices)?.raw.handle,
                        0,
                        vk::IndexType::UINT32,
                    );
                    device.cmd_draw_indexed(cmd, d.index_count, 1, d.first_index, 0, 0);
                }
                Command::Fullscreen => device.cmd_draw(cmd, 3, 1, 0, 0),
                Command::EndRendering => device.cmd_end_rendering(cmd),
                Command::UploadImage {
                    source,
                    destination,
                } => {
                    let i = self.images.get(*destination)?.raw;
                    device.cmd_copy_buffer_to_image(
                        cmd,
                        self.buffers.get(*source)?.raw.handle,
                        i.handle,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[copy_region(i)],
                    );
                }
                Command::CopyBuffer {
                    source,
                    destination,
                    size,
                } => {
                    device.cmd_copy_buffer(
                        cmd,
                        self.buffers.get(*source)?.raw.handle,
                        self.buffers.get(*destination)?.raw.handle,
                        &[vk::BufferCopy::default().size(*size)],
                    );
                }
                Command::ReadImage {
                    source,
                    destination,
                } => {
                    let i = self.images.get(*source)?.raw;
                    device.cmd_copy_image_to_buffer(
                        cmd,
                        i.handle,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        self.buffers.get(*destination)?.raw.handle,
                        &[copy_region(i)],
                    );
                }
                Command::BeginPass(_) | Command::EndPass => {
                    device.cmd_write_timestamp2(
                        cmd,
                        if matches!(command, Command::BeginPass(_)) {
                            vk::PipelineStageFlags2::TOP_OF_PIPE
                        } else {
                            vk::PipelineStageFlags2::ALL_COMMANDS
                        },
                        query,
                        *timestamp,
                    );
                    *timestamp += 1;
                }
            }
        }
        Ok(())
    }
}
unsafe fn viewport(device: &ash::Device, cmd: vk::CommandBuffer, origin: [u32; 2], size: [u32; 2]) {
    // SAFETY: Called only within a graphics command buffer, with positive attachment extent.
    unsafe {
        device.cmd_set_viewport(
            cmd,
            0,
            &[vk::Viewport {
                x: origin[0] as f32,
                y: origin[1] as f32,
                width: size[0] as f32,
                height: size[1] as f32,
                min_depth: 0.,
                max_depth: 1.,
            }],
        );
        device.cmd_set_scissor(
            cmd,
            0,
            &[vk::Rect2D {
                offset: vk::Offset2D {
                    x: origin[0] as i32,
                    y: origin[1] as i32,
                },
                extent: vk::Extent2D {
                    width: size[0],
                    height: size[1],
                },
            }],
        );
    }
}
fn copy_region(i: ImageAllocation) -> vk::BufferImageCopy {
    vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(i.aspect)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: i.width,
            height: i.height,
            depth: 1,
        })
}
