use super::image_copy::copy_bytes;
use super::*;
use crate::commands::*;

pub(super) struct Pending {
    pub point: TimelinePoint,
    pub cmd: vk::CommandBuffer,
    pub query: vk::QueryPool,
    pub query_capacity: u32,
    pub labels: Vec<String>,
}

pub(super) fn access(
    usage: Access,
    queue: Queue,
) -> (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout) {
    use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as S};
    let shader = if queue == Queue::Compute {
        S::COMPUTE_SHADER
    } else {
        S::ALL_GRAPHICS | S::COMPUTE_SHADER
    };
    match usage {
        Access::Undefined => (S::NONE, A::NONE, L::UNDEFINED),
        Access::Vertex => (
            S::VERTEX_ATTRIBUTE_INPUT,
            A::VERTEX_ATTRIBUTE_READ,
            L::GENERAL,
        ),
        Access::Index => (S::INDEX_INPUT, A::INDEX_READ, L::GENERAL),
        Access::Indirect => (
            S::DRAW_INDIRECT | shader,
            A::INDIRECT_COMMAND_READ | A::SHADER_READ,
            L::GENERAL,
        ),
        Access::ShaderRead => (shader, A::SHADER_READ, L::SHADER_READ_ONLY_OPTIMAL),
        Access::Storage => (shader, A::SHADER_READ | A::SHADER_WRITE, L::GENERAL),
        Access::Color => (
            S::COLOR_ATTACHMENT_OUTPUT,
            A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE,
            L::COLOR_ATTACHMENT_OPTIMAL,
        ),
        Access::Depth => (
            S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
            A::DEPTH_STENCIL_ATTACHMENT_READ | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
            L::DEPTH_ATTACHMENT_OPTIMAL,
        ),
        Access::CopySource => (S::ALL_TRANSFER, A::TRANSFER_READ, L::TRANSFER_SRC_OPTIMAL),
        Access::CopyDestination => (S::ALL_TRANSFER, A::TRANSFER_WRITE, L::TRANSFER_DST_OPTIMAL),
        Access::AsBuild => (
            S::ACCELERATION_STRUCTURE_BUILD_KHR,
            A::ACCELERATION_STRUCTURE_WRITE_KHR,
            L::GENERAL,
        ),
        Access::AsInput => (
            S::ACCELERATION_STRUCTURE_BUILD_KHR,
            // Builds read vertex/index/instance buffers as shader reads, and BLAS as AS reads.
            A::SHADER_READ | A::ACCELERATION_STRUCTURE_READ_KHR,
            L::GENERAL,
        ),
        Access::AsRead => (shader, A::ACCELERATION_STRUCTURE_READ_KHR, L::GENERAL),
        Access::HostRead => (S::HOST, A::HOST_READ, L::GENERAL),
        Access::HostWrite => (S::HOST, A::HOST_WRITE, L::GENERAL),
    }
}

impl VulkanDevice {
    /// Submit one list with timeline dependencies. Returns a monotonic signal point.
    /// Bindless reachability is conservative: all published resources are retained.
    pub fn submit(&mut self, list: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        self.poll_shader_reload();
        crate::cpu_scope!("GPU.submit");
        // A synchronous helper's resolve used to finish its writes before the
        // next helper could submit. Preserve that boundary with GPU waits when
        // resolution is deferred, including across independently compiled graphs.
        let mut merged = [TimelinePoint {
            queue: Queue::Graphics,
            value: 0,
        }; 2];
        let mut count = 0;
        if self.deferred.is_some() {
            let mut dependencies = self.deferred_waits;
            for &point in waits {
                dependencies[point.queue.index()] =
                    dependencies[point.queue.index()].max(point.value);
            }
            for queue in [Queue::Graphics, Queue::Compute] {
                let value = dependencies[queue.index()];
                if value > 0 {
                    merged[count] = TimelinePoint { queue, value };
                    count += 1;
                }
            }
        }
        let waits = if self.deferred.is_some() {
            &merged[..count]
        } else {
            waits
        };
        for wait in waits {
            if wait.value > self.submitted[wait.queue.index()] {
                return Err(GpuError::Contract("Wait targets unsubmitted work".into()));
            }
        }
        self.validate_commands(&list)?;
        let queue = list.queue;
        let cmd = self.context.commands(queue == Queue::Compute)?;
        let mut guard = super::guard::Objects::new(&self.context.device);
        guard.push(super::guard::Object::Commands(
            if queue == Queue::Compute {
                self.context.compute_pool
            } else {
                self.context.graphics_pool
            },
            cmd,
        ));
        let labels = list
            .commands
            .iter()
            .filter_map(|c| {
                if let Command::BeginPass(label) = c {
                    Some(label.to_owned())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let required = labels
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(2))
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                GpuError::Contract("Timestamp query count exceeds Vulkan limits".into())
            })?;
        let (query, query_capacity) = match self.query_cache.take(required) {
            Some(pool) => (pool.handle, pool.capacity),
            None => {
                crate::cpu_scope!("GPU.create_queries");
                let capacity = required.checked_next_power_of_two().unwrap_or(required);
                // SAFETY: Pending submission owns this pool until its timeline completes.
                let query = unsafe {
                    self.context.device.create_query_pool(
                        &vk::QueryPoolCreateInfo::default()
                            .query_type(vk::QueryType::TIMESTAMP)
                            .query_count(capacity),
                        None,
                    )
                }
                .map_err(anyhow::Error::from)?;
                #[cfg(test)]
                {
                    self.query_pool_creations += 1;
                }
                (query, capacity)
            }
        };
        guard.push(super::guard::Object::Query(query));
        // SAFETY: The newly allocated command buffer has never been submitted.
        unsafe {
            self.context.begin(cmd)?;
        }
        // SAFETY: A reused pool was resolved after completion; this buffer is fresh.
        unsafe {
            self.context
                .device
                .cmd_reset_query_pool(cmd, query, 0, required);
        }
        // The outer pair covers every command, including leading barriers and
        // lists without graph labels. Per-pass begins below exclude those barriers.
        // SAFETY: The fresh/reset pool reserves query zero and the final query
        // for this submission. TOP_OF_PIPE starts its whole-device-work span.
        unsafe {
            self.context.device.cmd_write_timestamp2(
                cmd,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                query,
                0,
            );
        }
        let mut timestamp = 1;
        {
            crate::cpu_scope!("GPU.record_commands");
            for command in super::timestamps::commands(&list.commands) {
                self.encode(cmd, queue, query, &mut timestamp, command)?;
            }
        }
        // SAFETY: Query `required-1` is reserved for the submission's completed
        // tail and is written only after all its commands have been encoded.
        unsafe {
            self.context.device.cmd_write_timestamp2(
                cmd,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                query,
                required - 1,
            );
        }
        let point = TimelinePoint {
            queue,
            value: self.submitted[queue.index()]
                .checked_add(1)
                .ok_or_else(|| GpuError::Contract("Queue timeline exhausted".into()))?,
        };
        let waits = waits
            .iter()
            .map(|p| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(self.timelines[p.queue.index()])
                    .value(p.value)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .collect::<Vec<_>>();
        let signals = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.timelines[queue.index()])
            .value(point.value)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let commands = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
        {
            crate::cpu_scope!("GPU.end_commands");
            // SAFETY: Encoding completed on this recording command buffer.
            unsafe {
                self.context
                    .device
                    .end_command_buffer(cmd)
                    .map_err(anyhow::Error::from)?;
            }
        }
        {
            crate::cpu_scope!("GPU.queue_submit");
            // SAFETY: Handles and queue usage were checked; waits name prior submissions.
            unsafe {
                self.context
                    .device
                    .queue_submit2(
                        if queue == Queue::Compute {
                            self.context.compute
                        } else {
                            if self.scene_queue {
                                self.context.scene_graphics
                            } else {
                                self.context.graphics
                            }
                        },
                        &[vk::SubmitInfo2::default()
                            .wait_semaphore_infos(&waits)
                            .signal_semaphore_infos(&signals)
                            .command_buffer_infos(&commands)],
                        vk::Fence::null(),
                    )
                    .map_err(anyhow::Error::from)?;
            }
        }
        guard.disarm();
        self.submitted[queue.index()] = point.value;
        // Conservative retention also covers resources reached indirectly through shader indices.
        for command in &list.commands {
            if let Command::BuildAccelerationStructure(h) = command {
                self.accelerations.get_mut(*h)?.raw.built = true;
            }
        }
        {
            crate::cpu_scope!("GPU.retain_bindless");
            for value in self.accelerations.values_mut() {
                value.last[queue.index()] = point.value;
            }
            for value in self.buffers.values_mut() {
                value.last[queue.index()] = point.value;
            }
            for value in self.images.values_mut() {
                value.last[queue.index()] = point.value;
            }
        }
        self.pending.push(Pending {
            point,
            cmd,
            query,
            query_capacity,
            labels,
        });
        if let Some(points) = &mut self.deferred {
            points.push(point);
        }
        for command in &list.commands {
            if let Command::Barrier(Barrier {
                resource: Resource::Buffer(buffer),
                handoff,
                ..
            }) = command
            {
                self.buffers.get_mut(*buffer)?.owner = handoff.map_or(queue, |h| h.to);
            }
        }
        Ok(point)
    }
    /// Wait, read timestamps, recycle its query pool and release recording storage.
    pub fn resolve(&mut self, point: TimelinePoint) -> Result<Vec<PassTiming>> {
        crate::cpu_scope!("GPU.resolve");
        if self.deferred.is_some() {
            if !self.pending.iter().any(|pending| pending.point == point) {
                return Err(GpuError::Contract("Unknown deferred submission".into()));
            }
            self.deferred_waits[point.queue.index()] =
                self.deferred_waits[point.queue.index()].max(point.value);
            return Ok(Vec::new());
        }
        self.wait(point)?;
        self.resolve_completed(point)
    }
    pub(super) fn resolve_completed(&mut self, point: TimelinePoint) -> Result<Vec<PassTiming>> {
        let index = self
            .pending
            .iter()
            .position(|p| p.point == point)
            .ok_or_else(|| GpuError::Contract("Submission already resolved".into()))?;
        let pending = self.pending.swap_remove(index);
        let mut cleanup = super::guard::Objects::new(&self.context.device);
        cleanup.push(super::guard::Object::Commands(
            if point.queue == Queue::Compute {
                self.context.compute_pool
            } else {
                self.context.graphics_pool
            },
            pending.cmd,
        ));
        cleanup.push(super::guard::Object::Query(pending.query));
        let mut stamps = vec![0u64; pending.labels.len() * 2 + 2];
        {
            crate::cpu_scope!("GPU.read_queries");
            // SAFETY: Timeline completion proves query availability and buffer idleness.
            unsafe {
                if !stamps.is_empty() {
                    self.context
                        .device
                        .get_query_pool_results(
                            pending.query,
                            0,
                            &mut stamps,
                            vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                        )
                        .map_err(anyhow::Error::from)?;
                }
            }
        }
        if let Some(handle) = cleanup.take_query(pending.query)
            && let Some(release) = self.query_cache.recycle(super::queries::Pool {
                handle,
                capacity: pending.query_capacity,
            })
        {
            cleanup.push(super::guard::Object::Query(release.handle));
        }
        {
            crate::cpu_scope!("GPU.free_recording");
            drop(cleanup);
        }
        let valid = self.context.info.timestamp_valid_bits[point.queue.index()];
        let reference = self.clock.reference(point.queue, valid, stamps[0])?;
        let period = self.context.info.timestamp_period_ns;
        self.timing_window.observe(
            point,
            reference.interval(stamps[0], stamps[stamps.len() - 1], period),
        );
        Ok(pending
            .labels
            .into_iter()
            .enumerate()
            .map(|(i, pass)| {
                let interval = reference.interval(stamps[i * 2 + 1], stamps[i * 2 + 2], period);
                PassTiming {
                    pass,
                    queue: point.queue,
                    milliseconds: interval.milliseconds(),
                    start_ms: interval.start_ms,
                    end_ms: interval.end_ms,
                }
            })
            .collect())
    }
    fn validate_attachment_formats(
        &self,
        bound: Option<Pipeline>,
        colors: &[vk::Format],
        depth: Option<vk::Format>,
    ) -> Result<()> {
        let handle = bound.ok_or_else(|| GpuError::Contract("No bound pipeline".into()))?;
        let pipeline = self.pipelines.get(handle)?;
        if pipeline.colors != colors || pipeline.depth != depth {
            return Err(GpuError::Contract(
                "Pipeline and rendering attachment formats differ".into(),
            ));
        }
        Ok(())
    }
    fn validate_commands(&self, list: &CommandList) -> Result<()> {
        crate::cpu_scope!("GPU.validate_commands");
        let mut rendering = false;
        let mut pass = false;
        let mut pipeline = None;
        let mut bound = None;
        let mut color_formats = Vec::new();
        let mut depth_format = None;
        let mut extent = [0u32; 2];
        for c in &list.commands {
            match c {
                Command::Barrier(_) if rendering => {
                    return Err(GpuError::Contract(
                        "Barriers must be outside rendering scopes".into(),
                    ));
                }
                Command::Barrier(b) => match b.resource {
                    Resource::Buffer(h) => {
                        self.buffers.get(h)?;
                    }
                    Resource::AccelerationStructure(h) => {
                        self.accelerations.get(h)?;
                    }
                    Resource::Image(h) => {
                        self.images.get(h)?;
                    }
                },
                Command::BuildAccelerationStructure(h) => {
                    if rendering {
                        return Err(GpuError::Contract("AS build inside rendering scope".into()));
                    }
                    self.validate_as_build(*h)?;
                }
                Command::Pipeline(h) => {
                    pipeline = Some(self.pipelines.get(*h)?.compute);
                    bound = Some(*h);
                    if list.queue == Queue::Compute && pipeline == Some(false) {
                        return Err(GpuError::Contract(
                            "Graphics pipeline on compute-only queue".into(),
                        ));
                    }
                }
                Command::BeginRendering {
                    colors,
                    depth,
                    size,
                } => {
                    if rendering || list.queue != Queue::Graphics || size.contains(&0) {
                        return Err(GpuError::Contract("Invalid rendering scope".into()));
                    }
                    for a in colors.iter().chain(depth.iter()) {
                        let i = self.images.get(a.image)?;
                        if size[0] > i.raw.width || size[1] > i.raw.height {
                            return Err(GpuError::Contract("Attachment extent too small".into()));
                        }
                    }
                    color_formats = colors
                        .iter()
                        .map(|a| self.images.get(a.image).map(|i| i.raw.format))
                        .collect::<Result<Vec<_>>>()?;
                    depth_format = depth
                        .as_ref()
                        .map(|a| self.images.get(a.image).map(|i| i.raw.format))
                        .transpose()?;
                    extent = *size;
                    rendering = true;
                }
                Command::ClearDepth {
                    origin,
                    size,
                    depth,
                } => {
                    let inside = (0..2).all(|i| {
                        origin[i]
                            .checked_add(size[i])
                            .is_some_and(|end| end <= extent[i])
                    });
                    if !rendering
                        || depth_format.is_none()
                        || size.contains(&0)
                        || !inside
                        || !depth.is_finite()
                    {
                        return Err(GpuError::Contract(
                            "Depth clear outside the rendering scope's depth attachment".into(),
                        ));
                    }
                }
                Command::EndRendering => {
                    if !rendering {
                        return Err(GpuError::Contract("No rendering scope".into()));
                    }
                    rendering = false;
                }
                Command::Draw(d) => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format)?;
                    let vertices = self.buffers.get(d.vertices)?;
                    if !vertices
                        .raw
                        .usage
                        .contains(vk::BufferUsageFlags::VERTEX_BUFFER)
                    {
                        return Err(GpuError::Contract("Vertex buffer usage missing".into()));
                    }
                    let i = self.buffers.get(d.indices)?;
                    if !i.raw.usage.contains(vk::BufferUsageFlags::INDEX_BUFFER) {
                        return Err(GpuError::Contract("Index buffer usage missing".into()));
                    }
                    if !rendering
                        || pipeline != Some(false)
                        || bound.is_some_and(|h| self.pipelines.get(h).is_ok_and(|p| p.mesh))
                        || (u64::from(d.first_index) + u64::from(d.index_count)) * 4 > i.raw.size
                    {
                        return Err(GpuError::Contract("Invalid indexed draw".into()));
                    }
                }
                Command::DrawIndirect { arguments, offset }
                | Command::DrawMeshTasksIndirect { arguments, offset } => {
                    let mesh_draw = matches!(c, Command::DrawMeshTasksIndirect { .. });
                    self.validate_attachment_formats(bound, &color_formats, depth_format)?;
                    let buffer = self.buffers.get(*arguments)?;
                    if !rendering
                        || pipeline != Some(false)
                        || bound.is_none_or(|h| {
                            self.pipelines
                                .get(h)
                                .is_ok_and(|p| p.vertex_stride != 0 || p.mesh != mesh_draw)
                        })
                        || offset % 4 != 0
                        || offset
                            .checked_add(if mesh_draw { 12 } else { 16 })
                            .is_none_or(|end| end > buffer.raw.size)
                        || !buffer
                            .raw
                            .usage
                            .contains(vk::BufferUsageFlags::INDIRECT_BUFFER)
                    {
                        return Err(GpuError::Contract("Invalid indirect draw".into()));
                    }
                }
                Command::Fullscreen => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format)?;
                    if !rendering
                        || pipeline != Some(false)
                        || bound.is_some_and(|h| self.pipelines.get(h).is_ok_and(|p| p.mesh))
                    {
                        return Err(GpuError::Contract("Draw outside graphics scope".into()));
                    }
                }
                Command::DrawProcedural {
                    vertices,
                    instances,
                } => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format)?;
                    let native = self
                        .pipelines
                        .get(bound.expect("attachment validation requires a bound pipeline"))?;
                    if !rendering
                        || pipeline != Some(false)
                        || native.vertex_stride != 0
                        || native.mesh
                        || *vertices == 0
                        || *instances == 0
                    {
                        return Err(GpuError::Contract(
                            "Invalid procedural graphics draw".into(),
                        ));
                    }
                }
                Command::Dispatch(_) => {
                    if rendering || pipeline != Some(true) {
                        return Err(GpuError::Contract("Dispatch outside compute scope".into()));
                    }
                }
                Command::DispatchIndirect { arguments, offset } => {
                    let buffer = self.buffers.get(*arguments)?.raw;
                    if rendering
                        || pipeline != Some(true)
                        || offset % 4 != 0
                        || offset.checked_add(12).is_none_or(|end| end > buffer.size)
                        || !buffer.usage.contains(vk::BufferUsageFlags::INDIRECT_BUFFER)
                    {
                        return Err(GpuError::Contract(
                            "Invalid indirect dispatch scope, usage or range".into(),
                        ));
                    }
                }
                Command::UploadImage {
                    source,
                    destination,
                } => {
                    let b = self.buffers.get(*source)?.raw;
                    let i = self.images.get(*destination)?.raw;
                    if rendering
                        || b.size < copy_bytes(i)?
                        || !b.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !i.usage.contains(vk::ImageUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid buffer-to-image copy".into()));
                    }
                }
                Command::CopyBuffer {
                    source,
                    destination,
                    size,
                } => {
                    let source = self.buffers.get(*source)?.raw;
                    let dest = self.buffers.get(*destination)?.raw;
                    if rendering
                        || *size == 0
                        || size % 4 != 0
                        || *size > source.size
                        || *size > dest.size
                        || !source.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !dest.usage.contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || source.handle == dest.handle
                    {
                        return Err(GpuError::Contract("Invalid buffer copy".into()));
                    }
                }
                Command::CopyBufferRange {
                    source,
                    source_offset,
                    destination,
                    destination_offset,
                    size,
                } => {
                    let source = self.buffers.get(*source)?.raw;
                    let destination = self.buffers.get(*destination)?.raw;
                    if rendering
                        || *size == 0
                        || size % 4 != 0
                        || source_offset % 4 != 0
                        || destination_offset % 4 != 0
                        || source_offset
                            .checked_add(*size)
                            .is_none_or(|end| end > source.size)
                        || destination_offset
                            .checked_add(*size)
                            .is_none_or(|end| end > destination.size)
                        || !source.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !destination
                            .usage
                            .contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || source.handle == destination.handle
                    {
                        return Err(GpuError::Contract("Invalid buffer copy range".into()));
                    }
                }
                Command::UpdateBuffer {
                    destination,
                    offset,
                    bytes,
                } => {
                    let destination = self.buffers.get(*destination)?.raw;
                    if rendering
                        || bytes.is_empty()
                        || bytes.len() > 65536
                        || bytes.len() % 4 != 0
                        || offset % 4 != 0
                        || offset
                            .checked_add(bytes.len() as u64)
                            .is_none_or(|end| end > destination.size)
                        || !destination
                            .usage
                            .contains(vk::BufferUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid buffer update".into()));
                    }
                }
                Command::CopyImage {
                    source,
                    destination,
                } => {
                    let source = self.images.get(*source)?.raw;
                    let destination = self.images.get(*destination)?.raw;
                    if rendering
                        || source.handle == destination.handle
                        || source.format != destination.format
                        || source.width != destination.width
                        || source.height != destination.height
                        || source.aspect != vk::ImageAspectFlags::COLOR
                        || !source.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                        || !destination
                            .usage
                            .contains(vk::ImageUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid image copy".into()));
                    }
                }
                Command::ReadImage {
                    source,
                    destination,
                } => {
                    let i = self.images.get(*source)?.raw;
                    let b = self.buffers.get(*destination)?.raw;
                    if rendering
                        || b.size < copy_bytes(i)?
                        || !b.usage.contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || !i.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                    {
                        return Err(GpuError::Contract("Invalid image-to-buffer copy".into()));
                    }
                }
                Command::BeginPass(_) => {
                    if pass {
                        return Err(GpuError::Contract("Nested measured pass".into()));
                    }
                    pass = true;
                }
                Command::EndPass => {
                    if !pass {
                        return Err(GpuError::Contract("No measured pass".into()));
                    }
                    pass = false;
                }
                _ => {}
            }
        }
        if pass || rendering {
            return Err(GpuError::Contract("Unclosed command scope".into()));
        }
        Ok(())
    }
}
