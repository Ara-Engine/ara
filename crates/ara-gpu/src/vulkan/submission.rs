//! A frame records helpers first, then submits consecutive work on each queue once.
use super::*;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Batch {
    pub entries: Vec<Entry>,
}
pub(super) struct Entry {
    pub point: TimelinePoint,
    pub cmd: vk::CommandBuffer,
    pub waits: [u64; 2],
}

/// Cumulative backend queue submissions, excluding unsafe native integrations/WSI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SubmissionStats {
    /// Successful vkQueueSubmit2 calls, one native submission per call.
    pub queue_submissions: u64,
    /// Original command lists, retaining their own timestamps and logical points.
    pub command_lists: u64,
}

impl VulkanDevice {
    /// Compare snapshots at frame boundaries to inspect native batching.
    pub fn submission_stats(&self) -> SubmissionStats {
        self.submission_stats
    }

    pub(super) fn check_submission(&self) -> Result<()> {
        if let Some(error) = &self.submission_error {
            return Err(anyhow::anyhow!("Submission owner failed: {error}").into());
        }
        Ok(())
    }

    pub(super) fn prepare_submission(&mut self, queue: Queue) -> Result<()> {
        self.check_submission()?;
        if self
            .batch
            .entries
            .first()
            .is_some_and(|e| e.point.queue != queue)
        {
            self.flush_batch()?;
        }
        Ok(())
    }

    pub(super) fn native_queue(&self, queue: Queue) -> vk::Queue {
        match queue {
            Queue::Compute => self.context.compute,
            Queue::Graphics if self.scene_queue => self.context.scene_graphics,
            Queue::Graphics => self.context.graphics,
        }
    }

    pub(super) fn flush_batch(&mut self) -> Result<()> {
        let Some(first) = self.batch.entries.first() else {
            return Ok(());
        };
        let queue = first.point.queue;
        let mut entries = std::mem::take(&mut self.batch.entries);
        let last = entries.last().expect("nonempty batch").point.value;
        let mut dependencies = [0; 2];
        for entry in &entries {
            for (value, wait) in dependencies.iter_mut().zip(entry.waits) {
                *value = (*value).max(wait);
            }
        }
        // Same-batch dependencies become full memory barriers between command
        // buffers. Only already-submitted work remains a semaphore dependency.
        dependencies[queue.index()] = dependencies[queue.index()].min(self.flushed[queue.index()]);
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
        let commands: Vec<_> = entries
            .iter()
            .map(|entry| vk::CommandBufferSubmitInfo::default().command_buffer(entry.cmd))
            .collect();
        let signals = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.timelines[queue.index()])
            .value(last)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        // SAFETY: This owner exclusively submits on the selected queue. Commands
        // and queries stay in pending until the batch's tail timeline completes.
        let result = unsafe {
            self.context.device.queue_submit2(
                self.native_queue(queue),
                &[vk::SubmitInfo2::default()
                    .wait_semaphore_infos(&waits)
                    .command_buffer_infos(&commands)
                    .signal_semaphore_infos(&signals)],
                vk::Fence::null(),
            )
        };
        #[cfg(test)]
        let result = if self.fail_batch {
            self.fail_batch = false;
            // The real submission above succeeded; failing after acceptance also
            // exercises the partial-submit drain before releasing recording data.
            Err(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)
        } else {
            result
        };
        match result {
            Ok(()) => {
                self.flushed[queue.index()] = last;
                self.submission_stats.queue_submissions += 1;
                self.submission_stats.command_lists += entries.len() as u64;
                entries.clear();
                self.batch.entries = entries;
                Ok(())
            }
            Err(error) => {
                self.submission_error = Some(error.to_string());
                // A failed native submit may have accepted part of its work.
                // Drain that queue before dropping recording objects; never retry
                // a device whose accepted timeline points are now ambiguous.
                // SAFETY: Same exclusive owner as submission; this is error cleanup.
                unsafe {
                    let _ = self
                        .context
                        .device
                        .queue_wait_idle(self.native_queue(queue));
                }
                self.discard_batch(queue);
                entries.clear();
                self.batch.entries = entries;
                Err(anyhow::Error::from(error).into())
            }
        }
    }

    fn discard_batch(&mut self, queue: Queue) {
        let q = queue.index();
        let tail = self.flushed[q];
        self.submitted[q] = tail;
        if let Some(points) = &mut self.deferred {
            points.retain(|point| point.queue != queue || point.value <= tail);
        }
        let mut index = 0;
        while index < self.pending.len() {
            let point = self.pending[index].point;
            if point.queue == queue && point.value > tail {
                let pending = self.pending.swap_remove(index);
                // SAFETY: Failed-batch cleanup drained this queue first.
                unsafe {
                    self.context.device.destroy_query_pool(pending.query, None);
                    self.context.device.free_command_buffers(
                        if queue == Queue::Compute {
                            self.context.compute_pool
                        } else {
                            self.context.graphics_pool
                        },
                        &[pending.cmd],
                    );
                }
            } else {
                index += 1;
            }
        }
        for value in self.buffers.values_mut() {
            value.last[q] = value.last[q].min(tail);
        }
        for value in self.images.values_mut() {
            value.last[q] = value.last[q].min(tail);
        }
        for value in self.accelerations.values_mut() {
            value.last[q] = value.last[q].min(tail);
        }
        for value in &mut self.retired {
            let last = match value {
                Retired::Buffer(v) => &mut v.last,
                Retired::Image(v) => &mut v.last,
                Retired::Acceleration(v) => &mut v.last,
                Retired::Pipeline(_, last) => last,
            };
            last[q] = last[q].min(tail);
        }
    }
}
