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
        self.read_hdr(gpu, FILTER0, false)
    }
    /// Read the separately accumulated direct/emissive signal for lighting diagnostics.
    /// Subtract it from `capture_radiance` to inspect filtered indirect transport.
    pub fn capture_direct_radiance(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        self.capture_signal(gpu, crate::targets::DIRECT_HISTORY0 + 1 - self.slot)
    }
    /// Read demodulated indirect history for offline temporal diagnostics.
    pub fn capture_incident_radiance(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        self.capture_signal(gpu, crate::targets::HISTORY0 + 1 - self.slot)
    }
    fn capture_signal(&self, gpu: &mut VulkanDevice, source: usize) -> Result<Vec<[f32; 4]>> {
        use ara_gpu::RootConstants;
        let size = self.settings.render_size;
        let image = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R32G32B32A32_SFLOAT,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let destination = gpu.storage_image(image)?.index;
        let mut c = gpu.begin(Queue::Graphics);
        c.barriers([
            Barrier {
                resource: Resource::Image(image),
                before: Access::Undefined,
                after: Access::Storage,
                handoff: None,
            },
            Barrier {
                resource: Resource::Image(self.targets.images[source]),
                before: self.targets.states[source].access,
                after: Access::ShaderRead,
                handoff: None,
            },
        ]);
        c.push(Command::Pipeline(self.pipelines["diagnostic"]));
        c.push(Command::Constants(RootConstants {
            words: [
                self.frame_index,
                0,
                self.targets.sampled[source],
                destination,
                0,
                0,
                0,
                0,
            ],
        }));
        c.push(Command::Dispatch([
            size[0].div_ceil(8),
            size[1].div_ceil(8),
            1,
        ]));
        c.barriers([Barrier {
            resource: Resource::Image(self.targets.images[source]),
            before: Access::ShaderRead,
            after: self.targets.states[source].access,
            handoff: None,
        }]);
        let p = gpu.submit(c.finish(), &[])?;
        gpu.resolve(p)?;
        let data = self.read_hdr_image(gpu, image, Access::Storage, false)?;
        gpu.retire(Resource::Image(image))?;
        gpu.collect()?;
        Ok(data)
    }
    fn read_hdr(&self, gpu: &mut VulkanDevice, target: usize, half: bool) -> Result<Vec<[f32; 4]>> {
        self.read_hdr_image(
            gpu,
            self.targets.images[target],
            self.targets.states[target].access,
            half,
        )
    }
    fn read_hdr_image(
        &self,
        gpu: &mut VulkanDevice,
        image: ara_gpu::Image,
        before: Access,
        half: bool,
    ) -> Result<Vec<[f32; 4]>> {
        anyhow::ensure!(self.history, "Render before capture");
        let size = self.settings.render_size;
        let stride = if half { 8 } else { 16 };
        let b = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * stride,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut c = gpu.begin(Queue::Graphics);
        c.barriers([
            Barrier {
                resource: Resource::Image(image),
                before,
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
                after: before,
                handoff: None,
            },
        ]);
        let p = gpu.submit(c.finish(), &[])?;
        gpu.resolve(p)?;
        let bytes = gpu.read(b)?;
        gpu.retire(Resource::Buffer(b))?;
        gpu.collect()?;
        Ok(bytes
            .chunks_exact(stride as usize)
            .map(|v| {
                std::array::from_fn(|i| {
                    if half {
                        decode_half(u16::from_le_bytes([v[i * 2], v[i * 2 + 1]]))
                    } else {
                        f32::from_le_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]])
                    }
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
fn decode_half(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = (bits >> 10) & 31;
    let mantissa = u32::from(bits & 1023);
    if exponent == 0 {
        (if sign == 0 { 1. } else { -1. }) * mantissa as f32 * (1. / 16777216.)
    } else {
        f32::from_bits(
            sign | (if exponent == 31 {
                255
            } else {
                u32::from(exponent) + 112
            } << 23)
                | (mantissa << 13),
        )
    }
}
