//! Explicit offline readbacks, outside the normal render loop.
use crate::{targets::FILTER0, view::FidelityView};
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, vk,
    vulkan::{Command, VulkanDevice},
};
impl FidelityView {
    /// Read histogram exposure diagnostics explicitly; ordinary frames do no readback.
    pub fn capture_exposure(&self, gpu: &mut VulkanDevice) -> Result<crate::ExposureState> {
        anyhow::ensure!(self.history, "Render before capture");
        let b = gpu.create_buffer(16, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let mut c = gpu.begin(Queue::Graphics);
        c.barriers([
            Barrier {
                resource: Resource::Buffer(self.exposure),
                before: self.exposure_state.access,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(b),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        c.push(Command::CopyBuffer {
            source: self.exposure,
            destination: b,
            size: 16,
        });
        c.barriers([
            Barrier {
                resource: Resource::Buffer(b),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(self.exposure),
                before: Access::CopySource,
                after: self.exposure_state.access,
                handoff: None,
            },
        ]);
        let point = gpu.submit(c.finish(), &[])?;
        gpu.resolve(point)?;
        let bytes = gpu.read(b)?;
        gpu.retire(Resource::Buffer(b))?;
        gpu.collect()?;
        let mut values = [0.; 4];
        for (i, word) in bytes.as_chunks::<4>().0.iter().enumerate() {
            values[i] = f32::from_le_bytes(*word);
        }
        Ok(crate::ExposureState {
            multiplier: values[0],
            previous_multiplier: values[1],
            target_ev100: values[2],
            metered_luminance: values[3],
        })
    }
    /// Read full-precision filtered linear radiance before exposure and display mapping.
    /// Reference mode copies its undenoised accumulation into the same channel.
    pub fn capture_radiance(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        anyhow::ensure!(self.history, "Render before capture");
        let size = self.settings.render_size;
        let image = self.targets.images[FILTER0];
        let b = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 16,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut c = gpu.begin(Queue::Graphics);
        c.barriers([
            Barrier {
                resource: Resource::Image(image),
                before: self.targets.states[FILTER0].access,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(b),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        c.push(Command::ReadImage {
            source: image,
            destination: b,
        });
        c.barriers([
            Barrier {
                resource: Resource::Buffer(b),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Image(image),
                before: Access::CopySource,
                after: self.targets.states[FILTER0].access,
                handoff: None,
            },
        ]);
        let p = gpu.submit(c.finish(), &[])?;
        gpu.resolve(p)?;
        let bytes = gpu.read(b)?;
        gpu.retire(Resource::Buffer(b))?;
        gpu.collect()?;
        Ok(bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|v| {
                std::array::from_fn(|i| {
                    f32::from_le_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]])
                })
            })
            .collect())
    }
    /// Explicit SDR readback used by the existing SSIM/golden infrastructure.
    pub fn capture(&self, gpu: &mut VulkanDevice) -> Result<image::RgbaImage> {
        anyhow::ensure!(self.history, "Render before capture");
        let size = self.settings.display_size;
        let b = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut c = gpu.begin(Queue::Graphics);
        c.push(Command::BeginPass("Capture.Readback".into()));
        c.barriers([Barrier {
            resource: Resource::Buffer(b),
            before: Access::Undefined,
            after: Access::CopyDestination,
            handoff: None,
        }]);
        c.push(Command::ReadImage {
            source: self.output(),
            destination: b,
        });
        c.barriers([Barrier {
            resource: Resource::Buffer(b),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        c.push(Command::EndPass);
        let p = gpu.submit(c.finish(), &[])?;
        gpu.resolve(p)?;
        let bytes = gpu.read(b)?;
        gpu.retire(Resource::Buffer(b))?;
        gpu.collect()?;
        image::RgbaImage::from_raw(size[0], size[1], bytes)
            .ok_or_else(|| anyhow::anyhow!("Malformed capture"))
    }
}
