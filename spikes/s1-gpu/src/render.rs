use crate::{
    gpu::{Buffer, Gpu},
    pipelines::{Constants, Pipelines},
    scene::Scene,
    window::{Present, Window},
};
use anyhow::{Result, ensure};
use ash::vk;
use ash::vk::TaggedStructure;
use serde::Serialize;

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;

/// Per-queue timestamp durations for a single deterministic frame.
#[derive(Serialize)]
pub struct FrameTiming {
    pub frame: u32,
    pub compute_ms: f64,
    pub raster_ray_query_ms: f64,
    pub readback_ms: f64,
}

/// Runtime evidence for actual compute outputs and timeline/ownership handoffs.
#[derive(Serialize)]
pub struct RenderResult {
    pub frames: Vec<FrameTiming>,
    pub compute_first_sample: f32,
    pub compute_last_sample: f32,
    pub compute_max_error: f32,
    pub last_timeline_values: [u64; 2],
    pub queue_family_transfers: u32,
    pub presentation: bool,
}

/// Deliberately serialized frames isolate correctness and timestamp measurements.
/// Queue-to-queue dependencies are still actual timeline waits and ownership pairs.
pub fn run(
    gpu: &mut Gpu,
    scene: &Scene,
    pipelines: &Pipelines,
    window: Option<&Window>,
    frame_count: u32,
    shadows: bool,
    output: &str,
) -> Result<RenderResult> {
    let color = gpu.image(
        WIDTH,
        HEIGHT,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::ImageAspectFlags::COLOR,
    )?;
    let depth = gpu.image(
        WIDTH,
        HEIGHT,
        vk::Format::D32_SFLOAT,
        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        vk::ImageAspectFlags::DEPTH,
    )?;
    let pixels = gpu.buffer(
        u64::from(WIDTH) * u64::from(HEIGHT) * 4,
        vk::BufferUsageFlags::TRANSFER_DST,
        true,
    )?;
    let data_readback = gpu.buffer(4096, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let pass = crate::passes::Pass {
        gpu,
        scene,
        pipelines,
        color,
        depth,
        pixels,
        data_readback,
    };
    let mut present = window.map(|w| Present::new(gpu, w)).transpose()?;
    let graphics_cmd = gpu.commands(false)?;
    let compute_cmd = gpu.commands(true)?;
    let mut first_sample = 0.;
    let mut last_sample = 0.;
    let mut max_error = 0_f32;
    let mut frames = Vec::with_capacity(frame_count as usize);
    let mut transfers = 0;
    // SAFETY: Query pools/semaphores are retained through the loop and destroyed
    // after device idle. Each command buffer is reset only after completion.
    unsafe {
        let compute_queries = gpu.device.create_query_pool(
            &vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(2),
            None,
        )?;
        let graphics_queries = gpu.device.create_query_pool(
            &vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(4),
            None,
        )?;
        let mut timeline_info = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        let compute_done = gpu.device.create_semaphore(
            &vk::SemaphoreCreateInfo::default().push(&mut timeline_info),
            None,
        )?;
        let graphics_done = gpu.device.create_semaphore(
            &vk::SemaphoreCreateInfo::default().push(&mut timeline_info),
            None,
        )?;
        for frame in 0..frame_count {
            if let Some(window) = window
                && !window.poll()
            {
                break;
            }
            let value = u64::from(frame) + 1;
            let buffer_index = frame % 4;
            let buffer = scene.material_buffers[buffer_index as usize];
            let constants = Constants {
                material: 0,
                texture: 0,
                buffer: buffer_index,
                flags: u32::from(shadows),
                phase: frame as f32 / 60.,
                aspect: WIDTH as f32 / HEIGHT as f32,
                padding: [0; 2],
            };
            gpu.begin(compute_cmd)?;
            gpu.device
                .cmd_reset_query_pool(compute_cmd, compute_queries, 0, 2);
            if frame >= 4 {
                let acquire = ownership(
                    buffer,
                    gpu.info.graphics_family,
                    gpu.info.compute_family,
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                );
                gpu.device.cmd_pipeline_barrier2(
                    compute_cmd,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[acquire]),
                );
                transfers += 1;
            }
            gpu.device.cmd_write_timestamp2(
                compute_cmd,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                compute_queries,
                0,
            );
            gpu.device.cmd_bind_pipeline(
                compute_cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipelines.compute,
            );
            gpu.device.cmd_bind_descriptor_sets(
                compute_cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipelines.layout,
                0,
                &[pipelines.set],
                &[],
            );
            gpu.device.cmd_push_constants(
                compute_cmd,
                pipelines.layout,
                all_stages(),
                0,
                bytemuck::bytes_of(&constants),
            );
            gpu.device.cmd_dispatch(compute_cmd, 4, 1, 1);
            gpu.device.cmd_write_timestamp2(
                compute_cmd,
                vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                compute_queries,
                1,
            );
            let release = ownership(
                buffer,
                gpu.info.compute_family,
                gpu.info.graphics_family,
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
            );
            gpu.device.cmd_pipeline_barrier2(
                compute_cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[release]),
            );
            gpu.device.end_command_buffer(compute_cmd)?;
            let compute_waits = if frame >= 4 {
                vec![
                    vk::SemaphoreSubmitInfo::default()
                        .semaphore(graphics_done)
                        .value(value - 4)
                        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
                ]
            } else {
                vec![]
            };
            let compute_signals = [vk::SemaphoreSubmitInfo::default()
                .semaphore(compute_done)
                .value(value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let compute_commands =
                [vk::CommandBufferSubmitInfo::default().command_buffer(compute_cmd)];
            gpu.device.queue_submit2(
                gpu.compute,
                &[vk::SubmitInfo2::default()
                    .wait_semaphore_infos(&compute_waits)
                    .signal_semaphore_infos(&compute_signals)
                    .command_buffer_infos(&compute_commands)],
                vk::Fence::null(),
            )?;

            let image_index = present.as_ref().map(Present::acquire).transpose()?;
            gpu.begin(graphics_cmd)?;
            gpu.device
                .cmd_reset_query_pool(graphics_cmd, graphics_queries, 0, 4);
            let acquire = ownership(
                buffer,
                gpu.info.compute_family,
                gpu.info.graphics_family,
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::TRANSFER_READ,
                ),
            );
            gpu.device.cmd_pipeline_barrier2(
                graphics_cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[acquire]),
            );
            transfers += 1;
            pass.record(graphics_cmd, graphics_queries, frame, constants, buffer);
            let release = ownership(
                buffer,
                gpu.info.graphics_family,
                gpu.info.compute_family,
                (
                    vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::TRANSFER_READ,
                ),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
            );
            gpu.device.cmd_pipeline_barrier2(
                graphics_cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[release]),
            );
            gpu.device.cmd_write_timestamp2(
                graphics_cmd,
                vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                graphics_queries,
                3,
            );
            if let (Some(present), Some(index)) = (&mut present, image_index) {
                present.record(gpu, graphics_cmd, color, index);
            }
            gpu.device.end_command_buffer(graphics_cmd)?;
            if frame == 0 {
                pipelines.update_after_bind(gpu, scene);
            }
            let mut waits = vec![
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(compute_done)
                    .value(value)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            ];
            let mut signals = vec![
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(graphics_done)
                    .value(value)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            ];
            if let (Some(present), Some(index)) = (&present, image_index) {
                waits.push(
                    vk::SemaphoreSubmitInfo::default()
                        .semaphore(present.acquire)
                        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
                );
                signals.push(
                    vk::SemaphoreSubmitInfo::default()
                        .semaphore(present.rendered(index))
                        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
                );
            }
            let commands = [vk::CommandBufferSubmitInfo::default().command_buffer(graphics_cmd)];
            gpu.device.queue_submit2(
                gpu.graphics,
                &[vk::SubmitInfo2::default()
                    .wait_semaphore_infos(&waits)
                    .signal_semaphore_infos(&signals)
                    .command_buffer_infos(&commands)],
                vk::Fence::null(),
            )?;
            if let (Some(present), Some(index)) = (&present, image_index) {
                present.present(gpu, index)?;
            }
            gpu.device.wait_semaphores(
                &vk::SemaphoreWaitInfo::default()
                    .semaphores(&[graphics_done])
                    .values(&[value]),
                u64::MAX,
            )?;
            let mut compute_times = [0_u64; 2];
            let mut graphics_times = [0_u64; 4];
            gpu.device.get_query_pool_results(
                compute_queries,
                0,
                &mut compute_times,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )?;
            gpu.device.get_query_pool_results(
                graphics_queries,
                0,
                &mut graphics_times,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )?;
            let scale = f64::from(gpu.info.timestamp_period_ns) / 1e6;
            frames.push(FrameTiming {
                frame,
                compute_ms: (compute_times[1] - compute_times[0]) as f64 * scale,
                raster_ray_query_ms: (graphics_times[1] - graphics_times[0]) as f64 * scale,
                readback_ms: (graphics_times[3] - graphics_times[2]) as f64 * scale,
            });
            let data = gpu.read(data_readback)?;
            for i in 0..256 {
                let offset = i * 16;
                let actual = f32::from_ne_bytes(data[offset..offset + 4].try_into()?);
                let expected = 0.88 + 0.12 * (i as f32 * 0.08 + constants.phase).sin();
                max_error = max_error.max((actual - expected).abs());
                if i == 0 {
                    last_sample = actual;
                    if frame == 0 {
                        first_sample = actual;
                    }
                }
            }
            ensure!(max_error < 0.00001, "Compute data mismatch: {max_error}");
            gpu.assert_clean()?;
        }
        ensure!(!frames.is_empty(), "Window closed before first frame");
        gpu.device.device_wait_idle()?;
        let bytes = gpu.read(pixels)?;
        let file = std::io::BufWriter::new(std::fs::File::create(output)?);
        let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&bytes)?;
        let timeline_values = [
            gpu.device.get_semaphore_counter_value(compute_done)?,
            gpu.device.get_semaphore_counter_value(graphics_done)?,
        ];
        gpu.device.destroy_semaphore(compute_done, None);
        gpu.device.destroy_semaphore(graphics_done, None);
        gpu.device.destroy_query_pool(compute_queries, None);
        gpu.device.destroy_query_pool(graphics_queries, None);
        if let Some(present) = &present {
            present.destroy(gpu);
        }
        Ok(RenderResult {
            frames,
            compute_first_sample: first_sample,
            compute_last_sample: last_sample,
            compute_max_error: max_error,
            last_timeline_values: timeline_values,
            queue_family_transfers: transfers,
            presentation: present.is_some(),
        })
    }
}

pub(crate) fn all_stages() -> vk::ShaderStageFlags {
    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE
}

fn ownership(
    buffer: Buffer,
    source: u32,
    dest: u32,
    source_access: (vk::PipelineStageFlags2, vk::AccessFlags2),
    dest_access: (vk::PipelineStageFlags2, vk::AccessFlags2),
) -> vk::BufferMemoryBarrier2<'static> {
    vk::BufferMemoryBarrier2::default()
        .buffer(buffer.handle)
        .offset(0)
        .size(buffer.size)
        .src_queue_family_index(source)
        .dst_queue_family_index(dest)
        .src_stage_mask(source_access.0)
        .src_access_mask(source_access.1)
        .dst_stage_mask(dest_access.0)
        .dst_access_mask(dest_access.1)
}
