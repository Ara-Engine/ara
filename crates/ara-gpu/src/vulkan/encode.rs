use super::image_copy::copy_regions;
use super::recording::access;
use super::*;
use crate::attachments::{Load, Ops};
use crate::commands::*;

impl VulkanDevice {
    pub(super) fn encode(
        &self,
        cmd: vk::CommandBuffer,
        queue: Queue,
        query: vk::QueryPool,
        timestamp: &mut u32,
        scopes: &mut std::vec::IntoIter<Vec<crate::attachments::Ops>>,
        command: &Command,
    ) -> Result<()> {
        let device = &self.context.device;
        // SAFETY: All handles were validated, scopes checked, and command buffer is recording.
        unsafe {
            match command {
                // A discard orders nothing; `attachments::ops` already consumed it, and
                // Vulkan has no transition to UNDEFINED.
                Command::Barrier(b) if b.after == Access::Undefined => {}
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
                            .dst_queue_family_index(to)
                            .subresource_range(
                                vk::ImageSubresourceRange::default()
                                    .aspect_mask(i.raw.aspect)
                                    .level_count(i.raw.mip_levels)
                                    .layer_count(1),
                            );
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
                Command::BeginRendering {
                    colors,
                    depth,
                    size,
                } => {
                    let ops = scopes
                        .next()
                        .expect("attachments::ops lists every rendering scope of the list");
                    let attachment = |a: &Attachment, depth: bool, ops: Ops| -> Result<_> {
                        Ok(vk::RenderingAttachmentInfo::default()
                            .image_view(self.images.get(a.image)?.raw.view)
                            .image_layout(if depth {
                                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL
                            } else {
                                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                            })
                            .load_op(match ops.load {
                                Load::Clear => vk::AttachmentLoadOp::CLEAR,
                                Load::Keep => vk::AttachmentLoadOp::LOAD,
                                Load::DontCare => vk::AttachmentLoadOp::DONT_CARE,
                            })
                            .store_op(if ops.store {
                                vk::AttachmentStoreOp::STORE
                            } else {
                                vk::AttachmentStoreOp::DONT_CARE
                            })
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
                    let depth = depth
                        .as_ref()
                        .map(|a| attachment(a, true, ops[colors.len()]))
                        .transpose()?;
                    let colors = colors
                        .iter()
                        .zip(&ops)
                        .map(|(a, ops)| attachment(a, false, *ops))
                        .collect::<Result<Vec<_>>>()?;
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
                Command::ClearDepth {
                    origin,
                    size,
                    depth,
                } => device.cmd_clear_attachments(
                    cmd,
                    &[vk::ClearAttachment {
                        aspect_mask: vk::ImageAspectFlags::DEPTH,
                        color_attachment: 0,
                        clear_value: vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: *depth,
                                stencil: 0,
                            },
                        },
                    }],
                    &[vk::ClearRect {
                        rect: vk::Rect2D {
                            offset: vk::Offset2D {
                                x: origin[0] as i32,
                                y: origin[1] as i32,
                            },
                            extent: vk::Extent2D {
                                width: size[0],
                                height: size[1],
                            },
                        },
                        base_array_layer: 0,
                        layer_count: 1,
                    }],
                ),
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
                Command::DrawIndirect { arguments, offset } => device.cmd_draw_indirect(
                    cmd,
                    self.buffers.get(*arguments)?.raw.handle,
                    *offset,
                    1,
                    16,
                ),
                Command::DrawMeshTasksIndirect { arguments, offset } => {
                    self.context.mesh.cmd_draw_mesh_tasks_indirect(
                        cmd,
                        self.buffers.get(*arguments)?.raw.handle,
                        *offset,
                        1,
                        12,
                    )
                }
                Command::Fullscreen => device.cmd_draw(cmd, 3, 1, 0, 0),
                Command::DrawProcedural {
                    vertices,
                    instances,
                } => device.cmd_draw(cmd, *vertices, *instances, 0, 0),
                Command::DrawIndexedProcedural {
                    indices,
                    first_index,
                    count,
                    instances,
                } => {
                    device.cmd_bind_index_buffer(
                        cmd,
                        self.buffers.get(*indices)?.raw.handle,
                        0,
                        vk::IndexType::UINT32,
                    );
                    device.cmd_draw_indexed(cmd, *count, *instances, *first_index, 0, 0);
                }
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
                        copy_regions(i)?.as_ref(),
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
                Command::CopyBufferRange {
                    source,
                    source_offset,
                    destination,
                    destination_offset,
                    size,
                } => {
                    device.cmd_copy_buffer(
                        cmd,
                        self.buffers.get(*source)?.raw.handle,
                        self.buffers.get(*destination)?.raw.handle,
                        &[vk::BufferCopy::default()
                            .src_offset(*source_offset)
                            .dst_offset(*destination_offset)
                            .size(*size)],
                    );
                }
                Command::UpdateBuffer {
                    destination,
                    offset,
                    bytes,
                } => {
                    let buffer = self.buffers.get(*destination)?.raw.handle;
                    let barrier = vk::BufferMemoryBarrier2::default()
                        .buffer(buffer)
                        .offset(*offset)
                        .size(bytes.len() as u64)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                    device.cmd_pipeline_barrier2(
                        cmd,
                        &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier
                            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                            .src_access_mask(
                                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                            )
                            .dst_stage_mask(vk::PipelineStageFlags2::CLEAR)
                            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)]),
                    );
                    device.cmd_update_buffer(cmd, buffer, *offset, bytes);
                    device.cmd_pipeline_barrier2(
                        cmd,
                        &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier
                            .src_stage_mask(vk::PipelineStageFlags2::CLEAR)
                            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                            .dst_access_mask(
                                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                            )]),
                    );
                }
                Command::CopyImage {
                    source,
                    destination,
                } => {
                    let source = self.images.get(*source)?.raw;
                    let destination = self.images.get(*destination)?.raw;
                    let layers = vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1);
                    device.cmd_copy_image(
                        cmd,
                        source.handle,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        destination.handle,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[vk::ImageCopy::default()
                            .src_subresource(layers)
                            .dst_subresource(layers)
                            .extent(vk::Extent3D {
                                width: source.width,
                                height: source.height,
                                depth: 1,
                            })],
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
                        copy_regions(i)?.as_ref(),
                    );
                }
                Command::BeginPass(_) | Command::EndPass => {
                    device.cmd_write_timestamp2(
                        cmd,
                        vk::PipelineStageFlags2::ALL_COMMANDS,
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
