//! Bounded asynchronous frame ownership; callers poll before reusing frame data.
use super::*;

/// All submissions made in one deferred-completion scope. Keep this until it
/// completes before rewriting frame resources or publishing its images to a UI.
#[must_use]
pub struct Completion {
    points: Vec<TimelinePoint>,
    device: vk::Device,
    resolved: bool,
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

    /// Defer timestamp resolution until `poll_completion`. Recording and submit
    /// still happen on the owner thread; there is no render thread or shared lock.
    /// Only one scope may be active, and its previous frame must have completed.
    pub fn begin_deferred_completion(&mut self) -> Result<()> {
        if self.deferred.is_some() || self.completion_pending {
            return Err(GpuError::Contract("Nested deferred completion".into()));
        }
        let completed = self.completed()?;
        if (0..2).any(|q| completed[q] < self.submitted[q]) {
            return Err(GpuError::Contract(
                "Previous frame is still in flight".into(),
            ));
        }
        self.scene_queue = true;
        self.deferred_waits = self.submitted;
        self.deferred = Some(Vec::new());
        Ok(())
    }

    /// End a scope even when frame preparation failed. Its submitted commands
    /// still own their resources until the returned completion has been drained.
    pub fn end_deferred_completion(&mut self) -> Result<Completion> {
        let points = self
            .deferred
            .take()
            .ok_or_else(|| GpuError::Contract("No deferred completion scope".into()))?;
        self.completion_pending = true;
        Ok(Completion {
            points,
            device: self.context.device.handle(),
            resolved: false,
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
        for point in frame.points.drain(..) {
            timings.extend(self.resolve_completed(point)?);
        }
        self.completion_pending = false;
        frame.resolved = true;
        self.publish_completed_to_ui()?;
        self.collect()?;
        self.assert_clean()?;
        Ok(Some(timings))
    }

    /// Explicit blocking boundary for capture, shutdown and resource reuse.
    /// Interactive hosts use `poll_completion` and keep the previous image.
    pub fn wait_completion(&mut self, frame: &mut Completion) -> Result<Vec<PassTiming>> {
        self.check_completion(frame)?;
        for &point in &frame.points {
            self.wait(point)?;
        }
        self.poll_completion(frame)?
            .ok_or_else(|| GpuError::Contract("Waited frame did not complete".into()))
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
