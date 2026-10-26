//! Bounded asynchronous frame ownership; callers poll before reusing frame data.
use super::*;

/// All submissions made in one deferred-completion scope. Keep this until it
/// completes before rewriting frame resources or publishing its images to a UI.
#[must_use]
pub struct Completion {
    points: Vec<TimelinePoint>,
    device: vk::Device,
    resolved: bool,
    failure: Option<String>,
}
impl Completion {
    /// Whether its submissions and query storage have been consumed. On a later
    /// diagnostic error, owners may still retire this completed frame's resources.
    pub fn is_resolved(&self) -> bool {
        self.resolved
    }
}

impl VulkanDevice {
    /// Whether the current owner is recording an asynchronous frame scope.
    pub fn deferring_completion(&self) -> bool {
        self.deferred.is_some()
    }
    /// Write frame constants without waiting for an earlier submission in this
    /// scope. In-flight writes become GPU-ordered updates; idle storage is mapped.
    pub fn upload_queued(&mut self, buffer: Buffer, bytes: &[u8]) -> Result<()> {
        self.upload_queued_range(buffer, 0, bytes)
    }

    /// Range form of `upload_queued`. GPU-ordered ranges are four-byte aligned.
    pub fn upload_queued_range(&mut self, buffer: Buffer, offset: u64, bytes: &[u8]) -> Result<()> {
        if self.deferred.is_none() {
            return self.upload_range(buffer, offset, bytes);
        }
        let b = self.buffers.get(buffer)?;
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < b.last[q]) {
            self.upload_ordered(buffer, offset, bytes)
        } else {
            self.context.upload_range(b.raw, offset, bytes)?;
            Ok(())
        }
    }

    /// Record an asynchronous frame on its owner thread. Consecutive same-queue
    /// helpers submit together at the scope's end (or a queue change); poll their
    /// retained completion before reusing storage. One frame may be active.
    pub fn begin_deferred_completion(&mut self) -> Result<()> {
        self.begin_completion(true)
    }

    /// Batch a synchronous presenter's helpers on their existing queue. End the
    /// scope and wait once before publishing; Scene/UI owners use the isolated
    /// `begin_deferred_completion` queue instead.
    pub fn begin_deferred_completion_on_current_queue(&mut self) -> Result<()> {
        self.begin_completion(false)
    }

    /// Number of logical command lists in this scope, for partitioning completed
    /// view/feature timings. Native submission counts may be smaller.
    pub fn deferred_submission_count(&self) -> usize {
        self.deferred.as_ref().map_or(0, Vec::len)
    }

    fn begin_completion(&mut self, isolate_scene: bool) -> Result<()> {
        self.check_submission()?;
        if self.deferred.is_some() || self.completion_pending {
            return Err(GpuError::Contract("Nested deferred completion".into()));
        }
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < self.submitted[q]) {
            return Err(GpuError::Contract(
                "Previous frame is still in flight".into(),
            ));
        }
        self.scene_queue |= isolate_scene;
        self.deferred_waits = self.submitted;
        self.deferred = Some(Vec::new());
        Ok(())
    }

    /// End a scope even when frame preparation failed. Its submitted commands
    /// still own their resources until the returned completion has been drained.
    pub fn end_deferred_completion(&mut self) -> Result<Completion> {
        if self.deferred.is_none() {
            return Err(GpuError::Contract("No deferred completion scope".into()));
        }
        // Even a failed flush returns the completion of earlier accepted work.
        // Its owner drains it and retires the rejected frame before seeing error.
        let _ = self.flush_batch();
        let points = self
            .deferred
            .take()
            .ok_or_else(|| GpuError::Contract("No deferred completion scope".into()))?;
        self.completion_pending = true;
        Ok(Completion {
            points,
            device: self.context.device.handle(),
            resolved: false,
            failure: self.submission_error.as_deref().map(str::to_owned),
        })
    }

    /// Poll without a device wait. None retains the frame and all its query pools;
    /// Some returns every pass timestamp and releases completed recording storage.
    pub fn poll_completion(&mut self, frame: &mut Completion) -> Result<Option<Vec<PassTiming>>> {
        self.check_completion(frame)?;
        if self.deferred.is_some() {
            return Err(GpuError::Contract(
                "Cannot poll an active frame scope".into(),
            ));
        }
        let completed = self.completed()?;
        if frame
            .points
            .iter()
            .any(|p| completed[p.queue.index()] < p.value)
        {
            return Ok(None);
        }
        let mut timings = Vec::new();
        self.consume_completion(frame, |_, passes| timings.extend(passes))?;
        Ok(Some(timings))
    }

    fn consume_completion(
        &mut self,
        frame: &mut Completion,
        mut consume: impl FnMut(TimelinePoint, Vec<PassTiming>),
    ) -> Result<()> {
        for point in frame.points.drain(..) {
            consume(point, self.resolve_completed(point)?);
        }
        self.completion_pending = false;
        frame.resolved = true;
        self.publish_completed_to_ui()?;
        self.collect()?;
        self.assert_clean()?;
        if let Some(error) = frame.failure.take() {
            return Err(anyhow::anyhow!("Frame submission failed: {error}").into());
        }
        Ok(())
    }

    /// Explicit blocking boundary for capture, shutdown and resource reuse.
    /// Interactive hosts use `poll_completion` and keep the previous image.
    pub fn wait_completion(&mut self, frame: &mut Completion) -> Result<Vec<PassTiming>> {
        let mut timings = Vec::new();
        self.wait_completion_with(frame, |_, passes| timings.extend(passes))?;
        Ok(timings)
    }

    /// Wait once at a frame boundary and deliver each submission's original pass
    /// timings in submission order. Owners can account view/feature ranges without
    /// copying labels or treating deferred helpers as zero-cost work.
    pub fn wait_completion_with(
        &mut self,
        frame: &mut Completion,
        consume: impl FnMut(TimelinePoint, Vec<PassTiming>),
    ) -> Result<()> {
        self.check_completion(frame)?;
        if self.deferred.is_some() {
            return Err(GpuError::Contract(
                "Cannot wait an active frame scope".into(),
            ));
        }
        let mut last = [0; 2];
        for point in &frame.points {
            last[point.queue.index()] = last[point.queue.index()].max(point.value);
        }
        for queue in [Queue::Graphics, Queue::Compute] {
            let value = last[queue.index()];
            if value > 0 {
                self.wait(TimelinePoint { queue, value })?;
            }
        }
        self.consume_completion(frame, consume)
    }

    /// Establish device memory visibility on the native UI/present queue after
    /// completion. The wait is GPU-side and covers only already-completed work.
    pub fn publish_completed_to_ui(&self) -> Result<()> {
        let completed = self.completed()?;
        if (0..2).any(|queue| completed[queue] < self.submitted[queue]) {
            return Err(GpuError::Contract(
                "Cannot publish an unfinished frame to the UI".into(),
            ));
        }
        if !self.scene_queue || self.context.scene_graphics == self.context.graphics {
            return Ok(());
        }
        let waits: Vec<_> = self
            .submitted
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0)
            .map(|(q, &value)| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(self.timelines[q])
                    .value(value)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .collect();
        // SAFETY: Both queues have the same exclusive owner thread. Timelines
        // and the UI queue belong to this device and outlive this submission.
        unsafe {
            self.context
                .device
                .queue_submit2(
                    self.context.graphics,
                    &[vk::SubmitInfo2::default().wait_semaphore_infos(&waits)],
                    vk::Fence::null(),
                )
                .map_err(anyhow::Error::from)?;
        }
        Ok(())
    }

    fn check_completion(&self, frame: &Completion) -> Result<()> {
        if frame.resolved || frame.device != self.context.device.handle() {
            return Err(GpuError::Contract(
                "Completion belongs to another or resolved frame".into(),
            ));
        }
        Ok(())
    }

    /// In-flight constants are updated by a GPU copy on their owning queue,
    /// never by overwriting mapped memory that a submitted shader may still read.
    pub(super) fn upload_ordered(
        &mut self,
        buffer: Buffer,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        let b = self.buffers.get(buffer)?;
        if !b.raw.host
            || !offset.is_multiple_of(4)
            || !bytes.len().is_multiple_of(4)
            || offset
                .checked_add(bytes.len() as u64)
                .is_none_or(|end| end > b.raw.size)
        {
            return Err(GpuError::Contract("Invalid ordered buffer upload".into()));
        }
        let queue = b.owner;
        let waits: Vec<_> = [Queue::Graphics, Queue::Compute]
            .into_iter()
            .filter_map(|queue| {
                let value = b.last[queue.index()];
                (value > 0).then_some(TimelinePoint { queue, value })
            })
            .collect();
        let mut commands = self.begin(queue);
        commands.push(Command::BeginPass("Frame.UploadConstants".into()));
        for (i, chunk) in bytes.chunks(65536).enumerate() {
            commands.push(Command::UpdateBuffer {
                destination: buffer,
                offset: offset + (i * 65536) as u64,
                bytes: chunk.to_vec(),
            });
        }
        commands.push(Command::EndPass);
        self.submit(commands.finish(), &waits)?;
        Ok(())
    }
}
