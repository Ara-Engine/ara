//! Host copies belong to captures and explicit picks, never ordinary live frames.
use super::*;
use ara_gpu::{Barrier, vulkan::Command};

impl Renderer {
    pub(super) fn readback(&mut self, color: bool) -> Result<()> {
        let channels = [
            (OUTPUT, self.targets.read_color),
            (ID, self.targets.read_ids),
        ];
        let channels = &channels[usize::from(!color)..];
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("CaptureReadback".into()));
        for &(image, destination) in channels {
            ensure!(
                self.targets.states[image].access == Access::CopySource
                    && self.targets.states[image].queue == Queue::Graphics,
                "Readback requires a completed render frame"
            );
            commands.barriers([Barrier {
                resource: Resource::Buffer(destination),
                before: Access::HostRead,
                after: Access::CopyDestination,
                handoff: None,
            }]);
            commands.push(Command::ReadImage {
                source: self.targets.images[image],
                destination,
            });
            commands.barriers([Barrier {
                resource: Resource::Buffer(destination),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            }]);
        }
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.last_timings.extend(self.gpu.resolve(point)?);
        Ok(())
    }
}
