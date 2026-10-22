use crate::{
    gpu::{Buffer, Gpu, Image, image_barrier},
    pipelines::{Constants, Pipelines},
    render::{HEIGHT, WIDTH, all_stages},
    scene::Scene,
};
use ash::vk;

/// Raster/ray-query and transfer pass recording, separate from queue scheduling.
pub struct Pass<'a> {
    pub gpu: &'a Gpu,
    pub scene: &'a Scene,
    pub pipelines: &'a Pipelines,
    pub color: Image,
    pub depth: Image,
    pub pixels: Buffer,
    pub data_readback: Buffer,
}

impl Pass<'_> {
    pub fn record(
        &self,
        graphics_cmd: vk::CommandBuffer,
        graphics_queries: vk::QueryPool,
        frame: u32,
        constants: Constants,
        buffer: Buffer,
    ) {
        let Self {
            gpu,
            scene,
            pipelines,
            color,
            depth,
            pixels,
            data_readback,
        } = *self;
        // SAFETY: Caller begins this graphics command buffer, acquires buffer
        // ownership and waits on compute. All referenced resources stay live.
        unsafe {
            let old_color = if frame == 0 {
                vk::ImageLayout::UNDEFINED
            } else {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            };
            let source = if frame == 0 {
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
            } else {
                (
                    vk::PipelineStageFlags2::COPY | vk::PipelineStageFlags2::BLIT,
                    vk::AccessFlags2::TRANSFER_READ,
                )
            };
            let color_barrier = image_barrier(
                color.handle,
                vk::ImageAspectFlags::COLOR,
                old_color,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                source,
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
            );
            let depth_barrier = image_barrier(
                depth.handle,
                vk::ImageAspectFlags::DEPTH,
                if frame == 0 {
                    vk::ImageLayout::UNDEFINED
                } else {
                    vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL
                },
                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
                (
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                ),
            );
            gpu.device.cmd_pipeline_barrier2(
                graphics_cmd,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(&[color_barrier, depth_barrier]),
            );
            gpu.device.cmd_write_timestamp2(
                graphics_cmd,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                graphics_queries,
                0,
            );
            let color_attachment = [vk::RenderingAttachmentInfo::default()
                .image_view(color.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: [0.032, 0.045, 0.065, 1.],
                    },
                })];
            let depth_attachment = vk::RenderingAttachmentInfo::default()
                .image_view(depth.view)
                .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .clear_value(vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 1.,
                        stencil: 0,
                    },
                });
            let area = vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: vk::Extent2D {
                    width: WIDTH,
                    height: HEIGHT,
                },
            };
            gpu.device.cmd_begin_rendering(
                graphics_cmd,
                &vk::RenderingInfo::default()
                    .render_area(area)
                    .layer_count(1)
                    .color_attachments(&color_attachment)
                    .depth_attachment(&depth_attachment),
            );
            gpu.device.cmd_set_viewport(
                graphics_cmd,
                0,
                &[vk::Viewport {
                    x: 0.,
                    y: 0.,
                    width: WIDTH as f32,
                    height: HEIGHT as f32,
                    min_depth: 0.,
                    max_depth: 1.,
                }],
            );
            gpu.device.cmd_set_scissor(graphics_cmd, 0, &[area]);
            gpu.device.cmd_bind_pipeline(
                graphics_cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipelines.graphics,
            );
            gpu.device.cmd_bind_descriptor_sets(
                graphics_cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipelines.layout,
                0,
                &[pipelines.set],
                &[],
            );
            gpu.device
                .cmd_bind_vertex_buffers(graphics_cmd, 0, &[scene.vertices.handle], &[0]);
            gpu.device.cmd_bind_index_buffer(
                graphics_cmd,
                scene.indices.handle,
                0,
                vk::IndexType::UINT32,
            );
            for draw in &scene.draws {
                let constants = Constants {
                    material: draw.material,
                    texture: draw.material,
                    ..constants
                };
                gpu.device.cmd_push_constants(
                    graphics_cmd,
                    pipelines.layout,
                    all_stages(),
                    0,
                    bytemuck::bytes_of(&constants),
                );
                gpu.device
                    .cmd_draw_indexed(graphics_cmd, draw.count, 1, draw.first_index, 0, 0);
            }
            gpu.device.cmd_end_rendering(graphics_cmd);
            gpu.device.cmd_write_timestamp2(
                graphics_cmd,
                vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                graphics_queries,
                1,
            );
            // Keep the next TOP timestamp from running ahead of the raster-end
            // BOTTOM timestamp; this spike measures the two passes in isolation.
            let boundary = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
            gpu.device.cmd_pipeline_barrier2(
                graphics_cmd,
                &vk::DependencyInfo::default().memory_barriers(&[boundary]),
            );
            gpu.device.cmd_write_timestamp2(
                graphics_cmd,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                graphics_queries,
                2,
            );
            let barrier = image_barrier(
                color.handle,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COPY | vk::PipelineStageFlags2::BLIT,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
            );
            gpu.device.cmd_pipeline_barrier2(
                graphics_cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
            );
            let copy = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: WIDTH,
                    height: HEIGHT,
                    depth: 1,
                });
            gpu.device.cmd_copy_image_to_buffer(
                graphics_cmd,
                color.handle,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                pixels.handle,
                &[copy],
            );
            gpu.device.cmd_copy_buffer(
                graphics_cmd,
                buffer.handle,
                data_readback.handle,
                &[vk::BufferCopy::default().size(buffer.size)],
            );
            let host_barriers = [host_read(pixels), host_read(data_readback)];
            gpu.device.cmd_pipeline_barrier2(
                graphics_cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&host_barriers),
            );
        }
    }
}

fn host_read(buffer: Buffer) -> vk::BufferMemoryBarrier2<'static> {
    vk::BufferMemoryBarrier2::default()
        .buffer(buffer.handle)
        .size(buffer.size)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ)
}
