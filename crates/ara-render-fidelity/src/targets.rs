use anyhow::Result;
use ara_gpu::{Access, Device, GpuDevice, Image, Queue, Resource, vk};
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

/// Bloom pass names in a frame, and when rerun over feature-composited radiance.
pub(crate) const BLOOM_NAMES: [&str; 9] = [
    "Bloom.Down.1",
    "Bloom.Down.2",
    "Bloom.Down.3",
    "Bloom.Down.4",
    "Bloom.Down.5",
    "Bloom.Up.4",
    "Bloom.Up.3",
    "Bloom.Up.2",
    "Bloom.Up.1",
];
pub(crate) const FEATURE_BLOOM_NAMES: [&str; 9] = [
    "Feature.Bloom.Down.1",
    "Feature.Bloom.Down.2",
    "Feature.Bloom.Down.3",
    "Feature.Bloom.Down.4",
    "Feature.Bloom.Down.5",
    "Feature.Bloom.Up.4",
    "Feature.Bloom.Up.3",
    "Feature.Bloom.Up.2",
    "Feature.Bloom.Up.1",
];
/// A bloom pass recipe: name, pass index, dispatch and graph uses.
pub(crate) type BloomPass = (
    &'static str,
    u32,
    [u32; 3],
    Vec<(ara_render::graph::ResourceId, Access)>,
);

/// The bloom pyramid's nine passes over a render size (see post.slang `bloomMain`),
/// with their levels packed into BLOOM0 (downsamples) and BLOOM1 (upsamples).
pub(crate) fn bloom_passes(
    names: &'static [&'static str; 9],
    render: [u32; 2],
    source: ara_render::graph::ResourceId,
    bloom: [ara_render::graph::ResourceId; 2],
    exposure: ara_render::graph::ResourceId,
) -> Vec<BloomPass> {
    (0..9u32)
        .map(|pass| {
            let level = if pass < 5 { pass + 1 } else { 9 - pass };
            let size = render.map(|v| (v >> level).max(1));
            let uses = match pass {
                0 => vec![
                    (source, Access::ShaderRead),
                    (exposure, Access::ShaderRead),
                    (bloom[0], Access::Storage),
                ],
                1..=4 => vec![(bloom[0], Access::Storage)],
                _ => vec![(bloom[0], Access::ShaderRead), (bloom[1], Access::Storage)],
            };
            (
                names[pass as usize],
                pass,
                [size[0].div_ceil(8), size[1].div_ceil(8), 1],
                uses,
            )
        })
        .collect()
}

pub(crate) struct Targets {
    pub images: Vec<Image>,
    pub sampled: [u32; COUNT],
    pub storage: [u32; COUNT],
    pub states: Vec<State>,
}
impl Targets {
    pub(crate) fn empty() -> Self {
        Self {
            images: Vec::new(),
            sampled: [0; COUNT],
            storage: [0; COUNT],
            states: Vec::new(),
        }
    }
    pub fn new(
        gpu: &mut GpuDevice,
        render: [u32; 2],
        display: [u32; 2],
        reference: bool,
    ) -> Result<Self> {
        let mut t = Self::empty();
        let allocation = (|| -> Result<()> {
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
                t.images.push(image);
                t.sampled[i] = gpu.sampled_image(image, false)?.index;
                t.storage[i] = gpu.storage_image(image)?.index;
                t.states.push(State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                });
            }
            Ok(())
        })();
        if let Err(error) = allocation {
            t.destroy(gpu)?;
            gpu.collect()?;
            return Err(error);
        }
        Ok(t)
    }
    /// Preserve the classic view's descriptor/memory footprint until split signals
    /// are requested. Public G-buffer and feature slots remain the first 28 images.
    pub fn enable_split(&mut self, gpu: &mut GpuDevice, size: [u32; 2]) -> Result<()> {
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
    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for i in self.images {
            gpu.retire(Resource::Image(i))?;
        }
        Ok(())
    }
}
