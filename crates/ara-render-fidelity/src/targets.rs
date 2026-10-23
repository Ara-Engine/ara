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
pub(crate) const COUNT: usize = 21;

pub(crate) struct Targets {
    pub images: Vec<Image>,
    pub sampled: [u32; 24],
    pub storage: [u32; 24],
    pub states: Vec<State>,
}
impl Targets {
    pub fn new(gpu: &mut VulkanDevice, render: [u32; 2], display: [u32; 2]) -> Result<Self> {
        let mut t = Self {
            images: Vec::new(),
            sampled: [0; 24],
            storage: [0; 24],
            states: Vec::new(),
        };
        for i in 0..COUNT {
            let size = if i >= UPSCALED { display } else { render };
            let image = gpu.create_image(
                size[0],
                size[1],
                if i == OUTPUT {
                    vk::Format::R8G8B8A8_UNORM
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
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for i in self.images {
            gpu.retire(Resource::Image(i))?;
        }
        Ok(())
    }
}
