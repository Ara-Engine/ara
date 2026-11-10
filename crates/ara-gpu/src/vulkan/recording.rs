mod validation;
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
        Access::Geometry => (
            S::INDEX_INPUT | shader,
            A::INDEX_READ | A::SHADER_READ,
            L::GENERAL,
        ),
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
    /// Within a deferred frame this records a logical point; end the scope before
    /// host waits. Consecutive same-queue lists share a native submission.
    pub fn submit(&mut self, list: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        if self.validate_commands(&list)? {
            self.prepare_render_views(&list)?;
        }
        let point = TimelinePoint {
            queue: list.queue,
            value: self.submitted[list.queue.index()] + 1,
        };
        let result = self.submit_staged(list, waits);
        if result.is_err() {
            self.cancel_staging(point);
        }
        result
    }

    fn submit_staged(
        &mut self,
        list: CommandList,
        waits: &[TimelinePoint],
    ) -> Result<TimelinePoint> {
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
        let queue = list.queue;
        self.prepare_submission(queue)?;
        let staging = self.stage_updates(&list)?;
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
        if self.deferred.is_some() && !self.batch.entries.is_empty() {
            // The old helper boundary waited on its predecessor at ALL_COMMANDS.
            // Preserve that memory dependency when both share one native submit.
            // SAFETY: Fresh command buffer outside rendering; this queue owns it.
            unsafe {
                let memory = [vk::MemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(
                        vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                    )];
                self.context.device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().memory_barriers(&memory),
                );
            }
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
            let mut scopes = crate::attachments::ops(&list.commands, |image| {
                self.images.get(image).is_ok_and(|i| {
                    i.raw
                        .usage
                        .contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT)
                })
            })
            .into_iter();
            for command in super::timestamps::commands(&list.commands) {
                self.encode(
                    cmd,
                    queue,
                    (query, &mut timestamp),
                    &mut scopes,
                    command,
                    staging.get(&(command as *const Command)).copied(),
                )?;
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
        let mut dependencies = [0; 2];
        for wait in waits {
            dependencies[wait.queue.index()] = dependencies[wait.queue.index()].max(wait.value);
        }
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
        if self.deferred.is_some() {
            self.batch.entries.push(super::submission::Entry {
                point,
                cmd,
                waits: dependencies,
            });
        } else {
            let waits: Vec<_> = dependencies
                .into_iter()
                .enumerate()
                .filter(|(_, value)| *value > 0)
                .map(|(q, value)| {
                    vk::SemaphoreSubmitInfo::default()
                        .semaphore(self.timelines[q])
                        .value(value)
                        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                })
                .collect();
            let signals = [vk::SemaphoreSubmitInfo::default()
                .semaphore(self.timelines[queue.index()])
                .value(point.value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let commands = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            crate::cpu_scope!("GPU.queue_submit");
            // SAFETY: Handles and queue usage were checked; waits name prior submissions.
            unsafe {
                self.context
                    .device
                    .queue_submit2(
                        self.native_queue(queue),
                        &[vk::SubmitInfo2::default()
                            .wait_semaphore_infos(&waits)
                            .signal_semaphore_infos(&signals)
                            .command_buffer_infos(&commands)],
                        vk::Fence::null(),
                    )
                    .map_err(anyhow::Error::from)?;
            }
            self.flushed[queue.index()] = point.value;
            self.submission_stats.queue_submissions += 1;
            self.submission_stats.command_lists += 1;
        }
        guard.disarm();
        self.submitted[queue.index()] = point.value;
        // Conservative retention also covers resources reached indirectly through shader indices.
        for command in &list.commands {
            if let Command::BuildAccelerationStructure(h) = command {
                self.mark_as_built(*h, queue)?;
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
            for dependency in command.barriers() {
                if let Resource::Buffer(buffer) = dependency.barrier.resource {
                    self.buffers.get_mut(buffer)?.owner =
                        dependency.barrier.handoff.map_or(queue, |h| h.to);
                }
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
        let interval = reference.interval(stamps[0], stamps[stamps.len() - 1], period);
        self.timing_window.observe(point, interval);
        timestamps::remember(&mut self.recent_spans, point, interval);
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
}
