//! Publication keeps the completed image alive through scene/target replacement.
use super::*;

impl Published {
    pub(super) fn copy(&self, gpu: &mut VulkanDevice, source: Image, fresh: bool) -> Result<f64> {
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("Presentation.Publish".into()));
        commands.barriers([Barrier {
            resource: Resource::Image(self.image),
            before: if fresh {
                Access::Undefined
            } else {
                Access::CopySource
            },
            after: Access::CopyDestination,
            handoff: None,
        }]);
        commands.push(Command::CopyImage {
            source,
            destination: self.image,
        });
        commands.barriers([Barrier {
            resource: Resource::Image(self.image),
            before: Access::CopyDestination,
            after: Access::CopySource,
            handoff: None,
        }]);
        commands.push(Command::EndPass);
        let point = gpu.submit(commands.finish(), &[])?;
        let elapsed = gpu
            .resolve(point)?
            .iter()
            .map(|pass| pass.milliseconds)
            .sum();
        gpu.assert_clean()?;
        Ok(elapsed)
    }
}

impl Presenter {
    pub(super) fn publish(&mut self, source: Image) -> Result<()> {
        let gpu = self
            .renderer
            .as_mut()
            .context("presentation device")?
            .gpu_mut();
        if let Some(published) = &self.published
            && published.size == self.size
        {
            self.stats.gpu_ms += published.copy(gpu, source, false)?;
            return Ok(());
        }
        let image = gpu.create_image(
            self.size[0],
            self.size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        let next = Published {
            image,
            size: self.size,
        };
        match next.copy(gpu, source, true) {
            Ok(time) => self.stats.gpu_ms += time,
            Err(error) => {
                gpu.retire(Resource::Image(image))?;
                return Err(error);
            }
        }
        if let Some(old) = self.published.replace(next) {
            gpu.retire(Resource::Image(old.image))?;
        }
        gpu.collect()?;
        Ok(())
    }
}
