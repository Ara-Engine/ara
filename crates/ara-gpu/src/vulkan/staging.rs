//! Completion-gated upload chunks, independent of public resource retention.
use super::*;
use crate::commands::{Command, CommandList};

pub(super) struct Chunk {
    raw: BufferAllocation,
    last: TimelinePoint,
}
impl VulkanDevice {
    pub(super) fn stage_updates(
        &mut self,
        list: &CommandList,
    ) -> Result<std::collections::HashMap<*const Command, vk::Buffer>> {
        if !list
            .commands
            .iter()
            .any(|c| matches!(c, Command::UpdateBuffer { bytes, .. } if bytes.len() > 65536))
        {
            return Ok(std::collections::HashMap::new());
        }
        let done = self.completed()?;
        let point = TimelinePoint {
            queue: list.queue,
            value: self.submitted[list.queue.index()] + 1,
        };
        let mut result = std::collections::HashMap::new();
        for command in &list.commands {
            let Command::UpdateBuffer { bytes, .. } = command else {
                continue;
            };
            if bytes.len() <= 65536 {
                continue;
            }
            let size = bytes.len() as u64;
            let index = match self.staging.iter().position(|c| {
                c.last.queue == list.queue
                    && c.raw.size >= size
                    && done[c.last.queue.index()] >= c.last.value
            }) {
                Some(index) => index,
                None => {
                    let raw = self.context.buffer(
                        size.max(256 * 1024).next_multiple_of(256 * 1024),
                        vk::BufferUsageFlags::TRANSFER_SRC,
                        true,
                    )?;
                    self.staging.push(Chunk { raw, last: point });
                    self.staging.len() - 1
                }
            };
            let chunk = &mut self.staging[index];
            self.context.upload_range(chunk.raw, 0, bytes)?;
            chunk.last = point;
            result.insert(command as *const Command, chunk.raw.handle);
        }
        Ok(result)
    }
    pub(super) fn cancel_staging(&mut self, point: TimelinePoint) {
        for chunk in &mut self.staging {
            if chunk.last == point {
                chunk.last.value = 0;
            }
        }
    }
}
