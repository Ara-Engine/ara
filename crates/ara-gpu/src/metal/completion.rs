//! Bounded asynchronous frame ownership, with the Vulkan backend's contract: one
//! scope at a time, at most `FRAMES_IN_FLIGHT` ended scopes unconsumed (the next
//! frame records while the last one runs), and callers poll or wait, boundedly when
//! an event pump must not stall, before reusing frame data. Scene and UI share the
//! graphics queue on Metal, so publishing a completed frame to the UI needs no extra
//! GPU dependency.
use super::MetalDevice;
use crate::commands::{Command, PassTiming};
use crate::{Buffer, Device, GpuError, Queue, Result, TimelinePoint};
use objc2_metal::MTLSharedEvent;

/// All submissions made in one deferred-completion scope. Keep this until it
/// completes before rewriting frame resources or publishing its images to a UI.
#[must_use]
pub struct Completion {
    points: Vec<TimelinePoint>,
    device: u64,
    resolved: bool,
    window: (Option<crate::commands::TimestampSpan>, Option<f64>),
}

impl Completion {
    /// Whether its submissions have been consumed.
    pub fn is_resolved(&self) -> bool {
        self.resolved
    }
    /// This frame's own device span once consumed (see the Vulkan completion).
    pub fn timing_window(&self) -> Option<crate::commands::TimestampSpan> {
        self.window.0
    }
    /// Gaps between this frame's submission intervals once consumed.
    pub fn timing_window_gaps(&self) -> Option<f64> {
        self.window.1
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
        if self.deferred.is_some() {
            return Err(GpuError::Contract("Nested deferred completion".into()));
        }
        if self.pending_completions >= crate::FRAMES_IN_FLIGHT {
            return Err(GpuError::Contract(
                "Frames in flight are at their limit; consume the oldest first".into(),
            ));
        }
        // With a pending frame, its own submissions are the work in flight.
        let completed = Device::completed(self)?;
        if self.pending_completions == 0 && (0..2).any(|q| completed[q] < self.submitted[q]) {
            return Err(GpuError::Contract(
                "Previous frame is still in flight".into(),
            ));
        }
        self.poll_shader_reload();
        self.deferred_waits = self.submitted;
        self.deferred = Some(Vec::new());
        Ok(())
    }
    /// End a scope even when frame preparation failed. Its submitted commands
    /// still own their resources until the returned completion has been drained.
    pub fn end_deferred_completion(&mut self) -> Result<Completion> {
        self.flush();
        let points = self
            .deferred
            .take()
            .ok_or_else(|| GpuError::Contract("No deferred completion scope".into()))?;
        self.pending_completions += 1;
        Ok(Completion {
            points,
            device: self.identity,
            resolved: false,
            window: (None, None),
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
    /// Waits for `point` at most `timeout`; false on timeout (an event pump's bound).
    pub fn wait_point_for(
        &self,
        point: TimelinePoint,
        timeout: std::time::Duration,
    ) -> Result<bool> {
        if point.value > self.submitted[point.queue.index()]
            || point.value > self.flushed[point.queue.index()]
        {
            return Err(GpuError::Contract(
                "Cannot wait on unsubmitted or unflushed work".into(),
            ));
        }
        let ms = timeout.as_millis().min(u128::from(u64::MAX)) as u64;
        let event = &self.events[point.queue.index()];
        Ok(self
            .waits
            .measure(|| event.waitUntilSignaledValue_timeoutMS(point.value, ms)))
    }
    /// `wait_completion_with`, waiting at most `timeout`; Ok(false) leaves `frame`
    /// pending (see the Vulkan completion).
    pub fn wait_completion_for(
        &mut self,
        frame: &mut Completion,
        timeout: std::time::Duration,
        consume: impl FnMut(TimelinePoint, Vec<PassTiming>),
    ) -> Result<bool> {
        self.check_completion(frame)?;
        if self.deferred.is_some() {
            return Err(GpuError::Contract(
                "Cannot wait an active frame scope".into(),
            ));
        }
        let deadline = std::time::Instant::now() + timeout;
        let mut last = [0; 2];
        for point in &frame.points {
            last[point.queue.index()] = last[point.queue.index()].max(point.value);
        }
        for queue in [Queue::Graphics, Queue::Compute] {
            let value = last[queue.index()];
            if value == 0 {
                continue;
            }
            if value > self.submitted[queue.index()] || value > self.flushed[queue.index()] {
                return Err(GpuError::Contract(
                    "Cannot wait on unsubmitted or unflushed work".into(),
                ));
            }
            let ms = deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            let event = &self.events[queue.index()];
            if !self
                .waits
                .measure(|| event.waitUntilSignaledValue_timeoutMS(value, ms))
            {
                return Ok(false);
            }
        }
        self.consume_completion(frame, consume)?;
        Ok(true)
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
        let points = std::mem::take(&mut frame.points);
        for &point in &points {
            consume(point, self.take_timings(point));
        }
        frame.window = crate::vulkan::timestamps::frame_window(&self.recent_spans, &points, true);
        self.pending_completions = self.pending_completions.saturating_sub(1);
        frame.resolved = true;
        // With another frame still in flight, the last one to complete publishes.
        if self.pending_completions == 0 {
            self.publish_completed_to_ui()?;
        }
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
