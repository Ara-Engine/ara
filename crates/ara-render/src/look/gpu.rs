//! Look resources: sky LUTs, the sky's IBL, froxel fog, the bloom pyramid and the
//! exposure state. They persist across frames with their last access recorded.
use super::{Look, frame::LookFrame, gi::GiGpu};
use crate::graph::{Graph, ResourceId, State};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Device, Image, Pipeline, Queue, Resource, vk, vulkan::VulkanDevice};
use std::path::Path;

pub(crate) const TRANSMITTANCE: usize = 0;
pub(crate) const MULTISCATTER: usize = 1;
pub(crate) const SKY_VIEW: usize = 2;
pub(crate) const FROXELS: usize = 3;
pub(crate) const SKY_IRRADIANCE: usize = 4;
pub(crate) const SKY_PREFILTER: usize = 5;
pub(crate) const BLOOM_DOWN: usize = 6;
pub(crate) const BLOOM_UP: usize = 7;
const IMAGES: usize = 8;

/// Fixed LUT extents; bloom atlases follow the render size.
fn extent(image: usize, size: [u32; 2]) -> [u32; 2] {
    match image {
        TRANSMITTANCE => [256, 64],
        MULTISCATTER => [32, 32],
        SKY_VIEW => [192, 108],
        FROXELS => [4096, 36],
        SKY_IRRADIANCE => [32, 16],
        SKY_PREFILTER => [256, 640],
        _ => [size[0], (size[1] / 2).max(1)],
    }
}

pub(crate) struct Pipelines {
    pub transmittance: Pipeline,
    pub multiscatter: Pipeline,
    pub sky_view: Pipeline,
    pub sky_irradiance: Pipeline,
    pub sky_prefilter: Pipeline,
    pub fog: Pipeline,
    pub exposure: Pipeline,
    pub bloom: Pipeline,
    pub post: Pipeline,
    pub gi_relight: Pipeline,
}

/// What the sky LUTs (`aerosols`) and the sky IBL (sun and sky) were computed for.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SkyKey {
    pub aerosols: f32,
    pub sun: [f32; 6],
}

pub(crate) struct LookGpu {
    pub look: Look,
    pub pipelines: Pipelines,
    pub frame: Buffer,
    pub frame_index: u32,
    pub state: Buffer,
    pub state_index: u32,
    pub state_access: Access,
    pub images: [Image; IMAGES],
    pub sampled: [u32; IMAGES],
    pub storage: [u32; IMAGES],
    pub states: [State; IMAGES],
    pub size: [u32; 2],
    /// Last computed sky; `None` until the first frame with a physical sky.
    pub sky: Option<SkyKey>,
    /// Lit frames stop before temporal resolve and post (`Renderer::finish`).
    pub deferred: bool,
    /// Probe-volume GI, when the scene has acquired probe surfaces.
    pub gi: Option<GiGpu>,
}

/// Graph identities of the look's persistent resources for one frame.
pub(crate) struct LookIds {
    pub images: [ResourceId; IMAGES],
    pub state: ResourceId,
    pub frame: ResourceId,
    /// Surface caches, statics, irradiance read and irradiance write.
    pub gi: Option<[ResourceId; 4]>,
}

impl LookGpu {
    /// Loads the cooked look pipelines (same source cohort as the renderer).
    pub(crate) fn new(
        gpu: &mut VulkanDevice,
        artifacts: &Path,
        content_key: &str,
        size: [u32; 2],
    ) -> Result<Self> {
        let mut load = |name: &str| -> Result<Pipeline> {
            let cooked: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
            ensure!(
                cooked.key == content_key,
                "Look cook is incomplete; recook the pipeline manifest"
            );
            Ok(gpu.create_pipeline(&cooked, artifacts)?)
        };
        let pipelines = Pipelines {
            transmittance: load("look_transmittance")?,
            multiscatter: load("look_multiscatter")?,
            sky_view: load("look_sky_view")?,
            sky_irradiance: load("look_sky_irradiance")?,
            sky_prefilter: load("look_sky_prefilter")?,
            fog: load("look_fog")?,
            exposure: load("look_exposure")?,
            bloom: load("look_bloom")?,
            post: load("look_post")?,
            gi_relight: load("look_gi_relight")?,
        };
        let frame = gpu.create_buffer(
            std::mem::size_of::<LookFrame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        let state = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, false)?;
        let state_index = gpu.storage_buffer(state)?.index;
        let mut images = Vec::with_capacity(IMAGES);
        let mut sampled = [0; IMAGES];
        let mut storage = [0; IMAGES];
        for (i, (s, w)) in sampled.iter_mut().zip(&mut storage).enumerate() {
            let [width, height] = extent(i, size);
            let image = gpu.create_image(
                width,
                height,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            *s = gpu.sampled_image(image, false)?.index;
            *w = gpu.storage_image(image)?.index;
            images.push(image);
        }
        Ok(Self {
            look: Look::default(),
            pipelines,
            frame,
            frame_index,
            state,
            state_index,
            state_access: Access::Undefined,
            images: images
                .try_into()
                .map_err(|_| anyhow::anyhow!("look image count"))?,
            sampled,
            storage,
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; IMAGES],
            size,
            sky: None,
            deferred: false,
            gi: None,
        })
    }

    /// Reallocates the render-size bloom atlases.
    pub(crate) fn resize(&mut self, gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<()> {
        if size == self.size {
            return Ok(());
        }
        for i in [BLOOM_DOWN, BLOOM_UP] {
            let [width, height] = extent(i, size);
            let image = gpu.create_image(
                width,
                height,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            let old = std::mem::replace(&mut self.images[i], image);
            gpu.retire(Resource::Image(old))?;
            self.sampled[i] = gpu.sampled_image(image, false)?.index;
            self.storage[i] = gpu.storage_image(image)?.index;
            self.states[i] = State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            };
        }
        self.size = size;
        Ok(())
    }

    /// Bindless indices for the shader record: (sampled, storage) rows of four.
    pub(crate) fn indices(&self) -> ([[u32; 4]; 2], [[u32; 4]; 2]) {
        let rows = |v: &[u32; IMAGES]| [[v[0], v[1], v[2], v[3]], [v[4], v[5], v[6], v[7]]];
        (rows(&self.sampled), rows(&self.storage))
    }

    /// Imports the persistent resources at their recorded states.
    pub(crate) fn import(&self, graph: &mut Graph) -> LookIds {
        LookIds {
            images: std::array::from_fn(|i| {
                graph.import(Resource::Image(self.images[i]), self.states[i])
            }),
            state: graph.import(
                Resource::Buffer(self.state),
                State {
                    access: self.state_access,
                    queue: Queue::Graphics,
                },
            ),
            frame: graph.import(
                Resource::Buffer(self.frame),
                State {
                    access: Access::HostWrite,
                    queue: Queue::Graphics,
                },
            ),
            gi: self.gi.as_ref().map(|gi| {
                let state = |access| State {
                    access,
                    queue: Queue::Graphics,
                };
                let (read, write) = (1 - gi.slot, gi.slot);
                [
                    graph.import(Resource::Buffer(gi.cache), state(Access::ShaderRead)),
                    graph.import(Resource::Buffer(gi.statics), state(Access::ShaderRead)),
                    graph.import(
                        Resource::Buffer(gi.irradiance[read]),
                        state(gi.access[read]),
                    ),
                    graph.import(
                        Resource::Buffer(gi.irradiance[write]),
                        state(gi.access[write]),
                    ),
                ]
            }),
        }
    }

    /// Records the final states of a compiled frame.
    pub(crate) fn finish(&mut self, ids: &LookIds, final_states: &[State]) {
        for (state, id) in self.states.iter_mut().zip(&ids.images) {
            *state = final_states[id.0];
        }
        self.state_access = final_states[ids.state.0].access;
        if let (Some(gi), Some(ids)) = (&mut self.gi, ids.gi) {
            let (read, write) = (1 - gi.slot, gi.slot);
            gi.access[read] = final_states[ids[2].0].access;
            gi.access[write] = final_states[ids[3].0].access;
        }
    }
}
