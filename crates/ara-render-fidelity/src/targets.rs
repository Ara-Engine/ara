use anyhow::Result;
use ara_gpu::{Access, Device, Image, Queue, Resource, vk, vulkan::VulkanDevice};
use ara_render::graph::State;

pub(crate) const WORLD0: usize = 0;
pub(crate) const NORMAL0: usize = 2;
pub(crate) const BASE: usize = 4;
pub(crate) const MOTION: usize = 5;
pub(crate) const DEPTH: usize = 6;
pub(crate) const RAW: usize = 7;
pub(crate) const HISTORY0: usize = 8;
pub(crate) const MOMENTS0: usize = 10;
pub(crate) const FILTER0: usize = 12;
pub(crate) const FILTER1: usize = 13;
pub(crate) const BLOOM0: usize = 14;
pub(crate) const BLOOM1: usize = 15;
pub(crate) const POST: usize = 16;
pub(crate) const UPSCALED: usize = 17;
pub(crate) const OUTPUT: usize = 18;
pub(crate) const DISPLAY_HISTORY0: usize = 19;
pub(crate) const DIRECT: usize = 21;
pub(crate) const DIRECT_HISTORY0: usize = 22;
pub(crate) const TRANSMITTANCE: usize = 24;
pub(crate) const MULTISCATTER: usize = 25;
pub(crate) const SKYVIEW: usize = 26;
pub(crate) const FROXELS: usize = 27;
pub(crate) const SPEC_RAW: usize = 28;
pub(crate) const SPEC_HISTORY0: usize = 29;
pub(crate) const SPEC_MOMENTS0: usize = 31;
pub(crate) const SPEC_FILTER0: usize = 33;
pub(crate) const SPEC_FILTER1: usize = 34;
pub(crate) const SURFACE_ALBEDO: usize = 35;
pub(crate) const SURFACE_GEOMETRY: usize = 36;
pub(crate) const COUNT: usize = 37;

pub(crate) struct Targets {
    pub images: Vec<Image>,
    pub sampled: [u32; COUNT],
    pub storage: [u32; COUNT],
    pub states: Vec<State>,
}
impl Targets {
    pub fn new(
        gpu: &mut VulkanDevice,
        render: [u32; 2],
        display: [u32; 2],
        reference: bool,
    ) -> Result<Self> {
        let mut t = Self {
            images: Vec::new(),
            sampled: [0; COUNT],
            storage: [0; COUNT],
            states: Vec::new(),
        };
        for i in 0..28 {
            let size = if (TRANSMITTANCE..28).contains(&i) {
                [[256, 64], [32, 32], [192, 108], [4096, 36]][i - TRANSMITTANCE]
            } else if (UPSCALED..=DISPLAY_HISTORY0 + 1).contains(&i) {
                display
            } else {
                render
            };
            let image = gpu.create_image(
                size[0],
                size[1],
                if i == OUTPUT {
                    vk::Format::R8G8B8A8_UNORM
                } else if !reference
                    && matches!(i, 2 | 3 | 7 | 8 | 9 | 13..=17 | 19..=23 | 28..=30 | 33..=34)
                {
                    vk::Format::R16G16B16A16_SFLOAT
                } else {
                    vk::Format::R32G32B32A32_SFLOAT
                },
                vk::ImageUsageFlags::SAMPLED
                    | if i == OUTPUT {
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                    } else {
                        vk::ImageUsageFlags::empty()
                    }
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )?;
            t.sampled[i] = gpu.sampled_image(image, false)?.index;
            t.storage[i] = gpu.storage_image(image)?.index;
            t.images.push(image);
            t.states.push(State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            });
        }
        Ok(t)
    }
    /// Preserve the classic view's descriptor/memory footprint until split signals
    /// are requested. Public G-buffer and feature slots remain the first 28 images.
    pub fn enable_split(&mut self, gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<()> {
        if self.images.len() == COUNT {
            return Ok(());
        }
        for i in self.images.len()..COUNT {
            let format = if (SPEC_MOMENTS0..SPEC_MOMENTS0 + 2).contains(&i) {
                vk::Format::R32G32B32A32_SFLOAT
            } else {
                vk::Format::R16G16B16A16_SFLOAT
            };
            let image = gpu.create_image(
                size[0],
                size[1],
                format,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )?;
            self.sampled[i] = gpu.sampled_image(image, false)?.index;
            self.storage[i] = gpu.storage_image(image)?.index;
            self.images.push(image);
            self.states.push(State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            });
        }
        Ok(())
    }
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for i in self.images {
            gpu.retire(Resource::Image(i))?;
        }
        Ok(())
    }
}
