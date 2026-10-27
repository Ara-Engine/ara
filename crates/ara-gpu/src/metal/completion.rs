//! Bounded asynchronous frame ownership, with the Vulkan backend's contract:
//! one scope at a time, its previous frame complete before it starts, and callers
//! poll before reusing frame data. Scene and UI share the graphics queue on Metal,
//! so publishing a completed frame to the UI needs no extra GPU dependency.
use super::MetalDevice;
use crate::commands::{Command, PassTiming};
use crate::{Buffer, Device, GpuError, Queue, Result, TimelinePoint};

/// All submissions made in one deferred-completion scope. Keep this until it
/// completes before rewriting frame resources or publishing its images to a UI.
#[must_use]
pub struct Completion {
    points: Vec<TimelinePoint>,
    device: u64,
    resolved: bool,
}

impl Completion {
    /// Whether its submissions have been consumed.
    pub fn is_resolved(&self) -> bool {
        self.resolved
    }
}

impl MetalDevice {
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
        let last = self.buffers.get(buffer)?.last;
        let completed = Device::completed(self)?;
        if (0..2).any(|q| completed[q] < last[q]) {
            self.upload_ordered(buffer, offset, bytes)
        } else {
            self.upload_range(buffer, offset, bytes)
        }
    }
    /// Start an asynchronous frame scope (scene work).
    pub fn begin_deferred_completion(&mut self) -> Result<()> {
        self.begin_completion()
    }
    /// Batch a synchronous presenter's helpers; identical to the scene scope here
    /// because Metal has one graphics queue for scene and UI work.
    pub fn begin_deferred_completion_on_current_queue(&mut self) -> Result<()> {
        self.begin_completion()
    }
    /// Number of submissions in the active scope; zero outside a scope.
    pub fn deferred_submission_count(&self) -> usize {
        self.deferred.as_ref().map_or(0, Vec::len)
    }
    fn begin_completion(&mut self) -> Result<()> {
        if self.deferred.is_some() || self.completion_pending {
            return Err(GpuError::Contract("Nested deferred completion".into()));
        }
        let completed = Device::completed(self)?;
        if (0..2).any(|q| completed[q] < self.submitted[q]) {
            return Err(GpuError::Contract(
                "Previous frame is still in flight".into(),
            ));
        }
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
            device: self.identity,
            resolved: false,
        })
    }
    /// Poll without waiting. None retains the frame; Some releases its recording
    /// storage and returns every pass timing.
    pub fn poll_completion(&mut self, frame: &mut Completion) -> Result<Option<Vec<PassTiming>>> {
        self.check_completion(frame)?;
        if self.deferred.is_some() {
            return Err(GpuError::Contract(
                "Cannot poll an active frame scope".into(),
            ));
        }
        let completed = Device::completed(self)?;
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
    /// Explicit blocking boundary for capture, shutdown and resource reuse.
    pub fn wait_completion(&mut self, frame: &mut Completion) -> Result<Vec<PassTiming>> {
        let mut timings = Vec::new();
        self.wait_completion_with(frame, |_, passes| timings.extend(passes))?;
        Ok(timings)
    }
    /// Wait once at a frame boundary and deliver each submission's pass timings in
    /// submission order.
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
                Device::wait(self, TimelinePoint { queue, value })?;
            }
        }
        self.consume_completion(frame, consume)
    }
    /// Check that all submitted work completed before a UI shows its images. Scene
    /// and UI share one Metal graphics queue, so no GPU dependency is added.
    pub fn publish_completed_to_ui(&self) -> Result<()> {
        let completed = Device::completed(self)?;
        if (0..2).any(|q| completed[q] < self.submitted[q]) {
            return Err(GpuError::Contract(
                "Cannot publish an unfinished frame to the UI".into(),
            ));
        }
        Ok(())
    }
    fn consume_completion(
        &mut self,
        frame: &mut Completion,
        mut consume: impl FnMut(TimelinePoint, Vec<PassTiming>),
    ) -> Result<()> {
        // Every point completed (checked by the callers); reclaim resolves them.
        self.reclaim()?;
        for point in std::mem::take(&mut frame.points) {
            consume(point, self.take_timings(point));
        }
        self.completion_pending = false;
        frame.resolved = true;
        self.publish_completed_to_ui()?;
        Device::collect(self)?;
        self.assert_clean()
    }
    fn check_completion(&self, frame: &Completion) -> Result<()> {
        if frame.resolved || frame.device != self.identity {
            return Err(GpuError::Contract(
                "Completion belongs to another or resolved frame".into(),
            ));
        }
        Ok(())
    }
    /// In-flight constants are updated by a GPU copy on their owning queue, never
    /// by overwriting shared memory that a submitted shader may still read.
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
