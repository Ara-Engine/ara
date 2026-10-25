//! Look resources: sky LUTs, the sky's IBL, froxel fog, the bloom pyramid and the
//! exposure state. Images are allocated only once the look uses them (the bindless
//! storage heap is shared with every view on the device) and persist across frames
//! with their last access recorded.
use super::{Look, Quality, frame::LookFrame, gi::GiGpu};
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
pub(crate) const AO_TRACE: usize = 8;
pub(crate) const AO_DENOISED: usize = 9;
pub(crate) const GI_DIFFUSE: usize = 10;
pub(crate) const GI_SPECULAR: usize = 11;
pub(crate) const SSR_PYRAMID: usize = 12;
pub(crate) const SSR_TRACE: usize = 13;
const IMAGES: usize = 14;

/// Fixed LUT extents; bloom atlases follow the render size.
fn extent(image: usize, size: [u32; 2]) -> [u32; 2] {
    match image {
        TRANSMITTANCE => [256, 64],
        MULTISCATTER => [32, 32],
        SKY_VIEW => [192, 108],
        FROXELS => [4096, 36],
        SKY_IRRADIANCE => [32, 16],
        SKY_PREFILTER => [256, 640],
        AO_TRACE | AO_DENOISED | GI_DIFFUSE | GI_SPECULAR | SSR_TRACE => {
            size.map(|v| v.div_ceil(2))
        }
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
    pub gi_scatter: Pipeline,
    pub ao_trace: Pipeline,
    pub ao_denoise: Pipeline,
    pub gi_gather: Pipeline,
    pub ssr_pyramid: Pipeline,
    pub ssr_trace: Pipeline,
}

/// What the sky LUTs (`aerosols`) and the sky IBL (sun and sky) were computed for.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SkyKey {
    pub aerosols: f32,
    pub sun: [f32; 6],
    pub clouds: [f32; 2],
}

pub(crate) struct LookGpu {
    pub look: Look,
    pub quality: Quality,
    pub pipelines: Pipelines,
    pub frame: Buffer,
    pub frame_index: u32,
    pub state: Buffer,
    pub state_index: u32,
    pub state_access: Access,
    pub images: [Option<Image>; IMAGES],
    pub sampled: [u32; IMAGES],
    pub storage: [u32; IMAGES],
    pub states: [State; IMAGES],
    pub size: [u32; 2],
    /// Last computed sky; `None` until the first frame with a physical sky.
    pub sky: Option<SkyKey>,
    /// Lit frames stop before temporal resolve and post (`Renderer::finish`).
    pub deferred: bool,
    /// The renderer history slot a deferred frame left for `finish` to resolve into.
    pub deferred_history: Option<usize>,
    /// Remaining frames whose exposure snaps to its meter (capture resets).
    pub settle: u32,
    /// Probe-volume GI, when the scene has acquired probe surfaces.
    pub gi: Option<GiGpu>,
    /// Distant shadows past the cascades (`Shadows::far`).
    pub far: Option<super::far::FarMap>,
    /// The cached cascade maps (`Shadows`).
    pub cascades: Option<super::cascades::Cascades>,
    /// Cached local-light shadows (`Quality::local_shadows`).
    pub local: Option<super::local::LocalMap>,
    /// Idle host staging for probe records, reused.
    pub probe_staging: Vec<super::gi::ProbeStaging>,
    /// Probe records uploaded for the next frame's `GI.Scatter`: staging, count.
    pub probe_writes: Vec<(super::gi::ProbeStaging, u32)>,
}

/// Graph identities of the look's persistent resources for one frame.
pub(crate) struct LookIds {
    pub images: [Option<ResourceId>; IMAGES],
    pub state: ResourceId,
    pub frame: ResourceId,
    /// Surface caches, meta, moments, irradiance read and irradiance write.
    pub gi: Option<[ResourceId; 5]>,
    /// The far shadow map.
    pub far: Option<ResourceId>,
    /// The cascade maps, in cascade order.
    pub cascades: Vec<ResourceId>,
    /// Staging of the probe writes this frame scatters (`LookGpu::probe_writes`).
    pub probe_writes: Vec<ResourceId>,
    /// The local-shadow atlas.
    pub local: Option<ResourceId>,
}

impl LookIds {
    /// A look image a declared pass uses; `LookGpu::ensure` allocated it.
    pub(crate) fn image(&self, i: usize) -> ResourceId {
        self.images[i].expect("look passes only use images their look allocated")
    }
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
            gi_scatter: load("look_gi_scatter")?,
            ao_trace: load("look_ao_trace")?,
            ao_denoise: load("look_ao_denoise")?,
            gi_gather: load("look_gi_gather")?,
            ssr_pyramid: load("look_ssr_pyramid")?,
            ssr_trace: load("look_ssr_trace")?,
        };
        let frame = gpu.create_buffer(
            std::mem::size_of::<LookFrame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        // Zeroed on the host so the first frame's history readers have a producer.
        let state = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.upload(state, &[0; 16])?;
        let state_index = gpu.storage_buffer(state)?.index;
        Ok(Self {
            look: Look::default(),
            quality: Quality::default(),
            pipelines,
            frame,
            frame_index,
            state,
            state_index,
            state_access: Access::HostWrite,
            images: [None; IMAGES],
            sampled: [0; IMAGES],
            storage: [0; IMAGES],
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; IMAGES],
            size,
            sky: None,
            deferred: false,
            deferred_history: None,
            settle: 0,
            gi: None,
            far: None,
            cascades: None,
            local: None,
            probe_staging: Vec::new(),
            probe_writes: Vec::new(),
        })
    }

    /// The look as this quality renders it (bloom off when the preset skips it).
    pub(crate) fn effective(&self) -> Look {
        Look {
            bloom: if self.quality.bloom {
                self.look.bloom
            } else {
                0.
            },
            ..self.look
        }
    }

    /// Drops the render-size bloom atlases; the next frame reallocates them.
    pub(crate) fn resize(&mut self, gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<()> {
        if size == self.size {
            return Ok(());
        }
        for i in [
            BLOOM_DOWN,
            BLOOM_UP,
            AO_TRACE,
            AO_DENOISED,
            GI_DIFFUSE,
            GI_SPECULAR,
            SSR_PYRAMID,
            SSR_TRACE,
        ] {
            if let Some(old) = self.images[i].take() {
                gpu.retire(Resource::Image(old))?;
            }
        }
        self.size = size;
        Ok(())
    }

    /// Allocates the images the current look needs (sky LUTs and IBL under a sky,
    /// froxels with sky or fog, bloom atlases with bloom). Never frees: looks rarely
    /// change, and a later frame would only allocate them again.
    pub(crate) fn ensure(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        let look = self.effective();
        let sky = look.sky.is_some();
        let needed = [
            sky,
            sky,
            sky,
            sky || look.fog.is_some(),
            sky,
            sky,
            look.bloom > 0.,
            look.bloom > 0.,
            self.quality.ambient_occlusion.is_some(),
            self.quality.ambient_occlusion.is_some(),
            self.gi.is_some(),
            self.gi.is_some(),
            self.quality.reflections.is_some(),
            self.quality.reflections.is_some(),
        ];
        for (i, needed) in needed.into_iter().enumerate() {
            if !needed || self.images[i].is_some() {
                continue;
            }
            let [width, height] = extent(i, self.size);
            let image = gpu.create_image(
                width,
                height,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            self.sampled[i] = gpu.sampled_image(image, false)?.index;
            self.storage[i] = gpu.storage_image(image)?.index;
            self.states[i] = State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            };
            self.images[i] = Some(image);
            if i == TRANSMITTANCE {
                // New LUTs must be computed even for an unchanged sky.
                self.sky = None;
            }
        }
        Ok(())
    }

    /// Bindless indices for the shader record: (sampled, storage) rows of four.
    /// Ambient occlusion rows: trace and denoised (sampled), then (storage).
    pub(crate) fn ao_indices(&self) -> [u32; 4] {
        [
            self.sampled[AO_TRACE],
            self.sampled[AO_DENOISED],
            self.storage[AO_TRACE],
            self.storage[AO_DENOISED],
        ]
    }

    /// Reflection rows: pyramid and trace (sampled), then (storage).
    pub(crate) fn ssr_indices(&self) -> [u32; 4] {
        [
            self.sampled[SSR_PYRAMID],
            self.sampled[SSR_TRACE],
            self.storage[SSR_PYRAMID],
            self.storage[SSR_TRACE],
        ]
    }

    /// Probe gather rows: diffuse and specular (sampled), then (storage).
    pub(crate) fn gather_indices(&self) -> [u32; 4] {
        [
            self.sampled[GI_DIFFUSE],
            self.sampled[GI_SPECULAR],
            self.storage[GI_DIFFUSE],
            self.storage[GI_SPECULAR],
        ]
    }

    pub(crate) fn indices(&self) -> ([[u32; 4]; 2], [[u32; 4]; 2]) {
        let rows = |v: &[u32; IMAGES]| [[v[0], v[1], v[2], v[3]], [v[4], v[5], v[6], v[7]]];
        // AO images use their own row (`ao_indices`).
        (rows(&self.sampled), rows(&self.storage))
    }

    /// Zeroes the exposure state so a capture converges exactly as from a fresh
    /// view (history then pre-exposes by the frame's exposure argument). An
    /// enclosing deferred Scene scope orders this reset behind earlier work.
    pub(crate) fn reset_exposure(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.upload_queued(self.state, &[0; 16])?;
        self.state_access = Access::HostWrite;
        Ok(())
    }

    /// Imports the persistent resources at their recorded states.
    pub(crate) fn import(&self, graph: &mut Graph) -> LookIds {
        LookIds {
            images: std::array::from_fn(|i| {
                self.images[i].map(|image| graph.import(Resource::Image(image), self.states[i]))
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
            cascades: self.cascades.as_ref().map_or_else(Vec::new, |c| {
                c.images
                    .iter()
                    .zip(&c.states)
                    .map(|(&image, &state)| graph.import(Resource::Image(image), state))
                    .collect()
            }),
            far: self
                .far
                .as_ref()
                .map(|far| graph.import(Resource::Image(far.image), far.state)),
            local: self
                .local
                .as_ref()
                .map(|local| graph.import(Resource::Image(local.atlas), local.state)),
            probe_writes: self
                .probe_writes
                .iter()
                .map(|(staging, _)| {
                    graph.import(
                        Resource::Buffer(staging.buffer),
                        State {
                            access: Access::HostWrite,
                            queue: Queue::Graphics,
                        },
                    )
                })
                .collect(),
            gi: self.gi.as_ref().map(|gi| {
                // A fresh volume's contents are undefined until `GI.Clear`.
                let state = |access| State {
                    access: if gi.fresh { Access::Undefined } else { access },
                    queue: Queue::Graphics,
                };
                let (read, write) = (1 - gi.slot, gi.slot);
                [
                    graph.import(Resource::Buffer(gi.cache), state(Access::ShaderRead)),
                    graph.import(Resource::Buffer(gi.meta), state(Access::ShaderRead)),
                    graph.import(Resource::Image(gi.moments), state(Access::ShaderRead)),
                    graph.import(Resource::Image(gi.irradiance[read]), state(gi.access[read])),
                    graph.import(
                        Resource::Image(gi.irradiance[write]),
                        state(gi.access[write]),
                    ),
                ]
            }),
        }
    }

    /// Records the final states of a compiled frame.
    pub(crate) fn finish(&mut self, ids: &LookIds, final_states: &[State]) {
        for (state, id) in self.states.iter_mut().zip(&ids.images) {
            if let Some(id) = id {
                *state = final_states[id.0];
            }
        }
        self.state_access = final_states[ids.state.0].access;
        if let (Some(gi), Some(ids)) = (&mut self.gi, ids.gi) {
            let (read, write) = (1 - gi.slot, gi.slot);
            gi.access[read] = final_states[ids[3].0].access;
            gi.access[write] = final_states[ids[4].0].access;
        }
        if let (Some(far), Some(id)) = (&mut self.far, ids.far) {
            far.state = final_states[id.0];
        }
        if let (Some(local), Some(id)) = (&mut self.local, ids.local) {
            local.state = final_states[id.0];
        }
        if let Some(cascades) = &mut self.cascades {
            for (state, id) in cascades.states.iter_mut().zip(&ids.cascades) {
                *state = final_states[id.0];
            }
        }
    }
}
