//! Completion and retained-image ownership shared by Scene and Game cameras.
use super::*;

impl Live {
    pub(in crate::renderer) fn replace_preallocated(
        &mut self,
        gpu: &mut VulkanDevice,
        images: Vec<Image>,
        size: [u32; 2],
        id_size: [u32; 2],
    ) -> Result<()> {
        ensure!(
            images.len() == 4 && !self.pending(),
            "Invalid retained target preparation"
        );
        for old in [self.spare.take(), self.reserved.take()]
            .into_iter()
            .flatten()
        {
            old.retire(gpu)?;
        }
        let output = |color, ids| Output {
            color,
            ids,
            size,
            id_size,
            labels: Vec::new(),
            epoch: None,
            written: false,
        };
        self.spare = Some(output(images[0], images[1]));
        self.reserved = Some(output(images[2], images[3]));
        Ok(())
    }
    pub(in crate::renderer) fn acquire(
        &mut self,
        gpu: &mut VulkanDevice,
        size: [u32; 2],
        id_size: [u32; 2],
    ) -> Result<Output> {
        while let Some(output) = self.spare.take().or_else(|| self.reserved.take()) {
            if output.size == size && output.id_size == id_size {
                return Ok(output);
            }
            output.retire(gpu)?;
        }
        Output::new(gpu, size, id_size)
    }

    pub(in crate::renderer) fn submitted(
        &mut self,
        completion: Completion,
        output: Output,
        publish: bool,
    ) {
        self.pending = Some(Pending {
            completion,
            output,
            publish,
        });
    }

    pub(in crate::renderer) fn poll(&mut self, gpu: &mut VulkanDevice) -> Result<bool> {
        if let Some(pending) = &mut self.pending {
            let timings = match gpu.poll_completion(&mut pending.completion) {
                Ok(Some(timings)) => timings,
                Ok(None) => return Ok(false),
                Err(error) => {
                    if pending.completion.is_resolved() {
                        self.pending
                            .take()
                            .expect("completed rejected frame")
                            .output
                            .retire(gpu)?;
                    }
                    return Err(error.into());
                }
            };
            self.completed(gpu, timings)?;
        }
        Ok(true)
    }

    pub(in crate::renderer) fn wait(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some(pending) = &mut self.pending {
            let timings = gpu.wait_completion(&mut pending.completion)?;
            self.completed(gpu, timings)?;
        }
        Ok(())
    }

    fn completed(&mut self, gpu: &mut VulkanDevice, timings: Vec<PassTiming>) -> Result<()> {
        let pending = self.pending.take().expect("completed pending frame");
        if pending.publish {
            self.completed_timings = Some(timings);
            self.spare = self.shown.replace(pending.output);
        } else {
            pending.output.retire(gpu)?;
        }
        Ok(())
    }

    pub(in crate::renderer) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        ensure!(!self.pending(), "Retained camera is still in flight");
        for output in [self.shown, self.spare, self.reserved]
            .into_iter()
            .flatten()
        {
            output.retire(gpu)?;
        }
        Ok(())
    }
}
