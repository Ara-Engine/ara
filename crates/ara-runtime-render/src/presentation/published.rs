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

    /// The submission that last copied into [`Self::output`]. On Apple the copy is not
    /// waited for: a device that samples the image orders itself after this point.
    pub fn point(&self) -> Option<ara_gpu::TimelinePoint> {
        self.point
    }

    /// Reads completed color into a capture while preserving its entity IDs,
    /// metadata and numbered overlays. Used only at an explicit capture boundary.
    /// `gpu` is the device that published this image.
    pub fn capture(
        &self,
        gpu: &mut GpuDevice,
        capture: &mut ara_frame::capture::Capture,
    ) -> Result<()> {
        ensure!(
            capture.image.width() == self.size[0] && capture.image.height() == self.size[1],
            "completed capture size differs from working view"
        );
        crate::readback::replace(gpu, self.image, self.size, capture)
    }

    /// Publishes a completed CopySource image. A failed replacement keeps `slot`
    /// intact; hosts can resize or replace working views without losing this image.
    pub fn publish(
        slot: &mut Option<Self>,
        gpu: &mut GpuDevice,
        source: Image,
        size: [u32; 2],
    ) -> Result<f64> {
        if let Some(published) = slot.as_mut()
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
        let mut next = Self {
            image,
            size,
            point: None,
        };
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
    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.image))?;
        Ok(())
    }

    /// Copies `source` into the retained image and returns the copy's GPU milliseconds. On
    /// Apple the copy is not waited for (0 is returned): the scene's next frame may already
    /// be drawing on the same device, and the UI that samples the image orders itself after
    /// the copy's point (`Self::point`). Elsewhere it waits, as the UI shares the queue.
    pub(super) fn copy(&mut self, gpu: &mut GpuDevice, source: Image, fresh: bool) -> Result<f64> {
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
        self.point = Some(point);
        #[cfg(target_vendor = "apple")]
        let elapsed = 0.;
        #[cfg(not(target_vendor = "apple"))]
        let elapsed = gpu
            .resolve(point)?
            .iter()
            .map(|pass| pass.milliseconds)
            .sum();
        // Scene frames submit this copy on `scene_graphics`. The editor UI samples
        // the image on the other queue and requires TRANSFER_SRC.
        gpu.publish_submission_to_ui(point)?;
        gpu.assert_clean()?;
        Ok(elapsed)
    }
}
