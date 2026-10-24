//! Publication keeps the completed image alive through scene/target replacement.
use super::*;

impl PresentationImage {
    /// Last completely copied output, independent of the renderer's working targets.
    pub fn output(&self) -> Image {
        self.image
    }

    /// Extent of the retained output, including after an unsuccessful resize.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Reads completed color into a capture while preserving its entity IDs,
    /// metadata and numbered overlays. Used only at an explicit capture boundary.
    pub fn capture(
        &self,
        renderer: &mut Renderer,
        capture: &mut ara_render::capture::Capture,
    ) -> Result<()> {
        ensure!(
            renderer.size() == self.size,
            "completed capture size differs from working view"
        );
        crate::readback::replace(renderer, self.image, capture)
    }

    /// Publishes a completed CopySource image. A failed replacement keeps `slot`
    /// intact; hosts can resize or replace working views without losing this image.
    pub fn publish(
        slot: &mut Option<Self>,
        gpu: &mut VulkanDevice,
        source: Image,
        size: [u32; 2],
    ) -> Result<f64> {
        if let Some(published) = slot
            && published.size == size
        {
            return published.copy(gpu, source, false);
        }
        let image = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        let next = Self { image, size };
        let elapsed = match next.copy(gpu, source, true) {
            Ok(time) => time,
            Err(error) => {
                next.destroy(gpu)?;
                return Err(error);
            }
        };
        if let Some(old) = slot.replace(next) {
            old.destroy(gpu)?;
        }
        gpu.collect()?;
        Ok(elapsed)
    }

    /// Releases the retained image before its owning device is destroyed.
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.image))?;
        Ok(())
    }

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
        let elapsed = Published::publish(&mut self.published, gpu, source, self.size)?;
        self.stats.gpu_ms += elapsed;
        if let Some(costs) = &mut self.stats.gpu {
            costs.publication_ms = elapsed;
        }
        Ok(())
    }
}
