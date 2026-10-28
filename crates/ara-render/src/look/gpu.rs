//! Look resources: sky LUTs, the sky's IBL, froxel fog, the bloom pyramid and the
//! exposure state. Images are allocated only once the look uses them (the bindless
//! storage heap is shared with every view on the device) and persist across frames
//! with their last access recorded.
use super::{Look, Quality, frame::LookFrame, gi::GiGpu};
use crate::graph::{Graph, ResourceId, State};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Pipeline, Queue, Resource, vk};
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
/// Projected virtual shadow page visibility (`look::vsm`) and the soft filter's
/// half-width (next frame's marking reach), RG16F at render size.
pub(crate) const VSM_MASK: usize = 14;
/// Depth of field (`look::lens`): half-resolution colour and signed radius, the
/// half-resolution gather, and the render-size composite the temporal resolve reads.
pub(crate) const DOF_PREPARED: usize = 15;
pub(crate) const DOF_GATHERED: usize = 16;
pub(crate) const DOF_COMPOSITE: usize = 17;
/// Motion blur velocity maxima per 16-pixel display tile and per 3x3 neighbourhood.
pub(crate) const MOTION_TILES: usize = 18;
pub(crate) const MOTION_NEIGHBOURS: usize = 19;
const IMAGES: usize = 20;
/// Depth of a visibility-bitmask sample (`LookGpu::bitmask_ao`) behind its front, as
/// a share of the AO radius: thinner foreground hides less of what lies behind it.
const BITMASK_THICKNESS: f32 = 0.4;
/// The largest colour lookup table: 32 steps per channel.
pub(crate) const LUT_SIZE: u32 = 32;
/// The exposure state (four floats), then the metering histogram's 256 counts
/// (look-post.slang exposureMain).
pub(crate) const STATE_BYTES: usize = 16 + 256 * 4;

/// Fixed LUT extents; the half-resolution effects and reflection pyramid follow
/// the render size, the bloom atlases the display size.
fn extent(image: usize, render: [u32; 2], display: [u32; 2]) -> [u32; 2] {
    match image {
        TRANSMITTANCE => [256, 64],
        MULTISCATTER => [32, 32],
        SKY_VIEW => [192, 108],
        FROXELS => [4096, 36],
        SKY_IRRADIANCE => [32, 16],
        SKY_PREFILTER => [256, 640],
        AO_TRACE | AO_DENOISED | GI_DIFFUSE | GI_SPECULAR | SSR_TRACE => {
            render.map(|v| v.div_ceil(2))
        }
        SSR_PYRAMID => [render[0], (render[1] / 2).max(1)],
        VSM_MASK | DOF_COMPOSITE => render,
        DOF_PREPARED | DOF_GATHERED => render.map(|v| v.div_ceil(2)),
        MOTION_TILES | MOTION_NEIGHBOURS => display.map(|v| v.div_ceil(16)),
        _ => [display[0], (display[1] / 2).max(1)],
    }
}

#[derive(Clone, Copy)]
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
    /// The post pass with motion blur (look-post.slang `postMotionMain`).
    pub post_motion: Pipeline,
    pub dof: Pipeline,
    pub motion: Pipeline,
    pub gi_relight: Pipeline,
    pub gi_scatter: Pipeline,
    pub ao_trace: Pipeline,
    pub ao_denoise: Pipeline,
    /// The High/Epic tier's visibility-bitmask occlusion and bounce (`bitmask_ao`).
    pub ao_bitmask: Pipeline,
    pub ao_bitmask_denoise: Pipeline,
    pub gi_gather: Pipeline,
    pub ssr_pyramid: Pipeline,
    pub ssr_trace: Pipeline,
    /// Reflection capture bake (face projection, scratch mips, GGX prefilter) and
    /// the per-frame relight (`look::captures`, look-captures.slang).
    pub capture_face: Pipeline,
    pub capture_mip: Pipeline,
    pub capture_prefilter: Pipeline,
    pub capture_refit: Pipeline,
    pub capture_tiles: Pipeline,
    pub horizon_sun: Pipeline,
    pub horizon_sky: Pipeline,
    pub vsm_mark: Pipeline,
    pub vsm_project: Pipeline,
    pub gi_relight_vsm: Pipeline,
    /// The refraction copy and its blurred levels (`look::refract`).
    pub refract: Pipeline,
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
    /// The colour lookup table (`Renderer::set_color_lut`): up to `LUT_SIZE`³ RGBA8
    /// texels, host-visible and allocated with the look, so setting a table never
    /// allocates. Secondary views share the renderer's.
    pub lut: Buffer,
    pub lut_index: u32,
    /// Steps per channel of the current table; zero without one.
    pub lut_size: u32,
    pub images: [Option<Image>; IMAGES],
    pub sampled: [u32; IMAGES],
    pub storage: [u32; IMAGES],
    pub states: [State; IMAGES],
    /// Render size of the scene passes.
    pub render: [u32; 2],
    /// Display size of the post chain.
    pub display: [u32; 2],
    /// Last computed sky; `None` until the first frame with a physical sky.
    pub sky: Option<SkyKey>,
    /// Lit frames stop before temporal resolve and post (`Renderer::finish`).
    pub deferred: bool,
    /// This frame blurs focus and motion (`blurs`): the graph and the recorded passes
    /// agree on it.
    pub frame_dof: bool,
    pub frame_motion: bool,
    /// This frame draws triangle edges (`ViewMode::wire`): the renderer composites
    /// them after the post chain, here or in a deferred frame's `finish`.
    pub frame_wire: bool,
    /// Frames are not shown (`Renderer::set_unseen`); probe volumes inherit it.
    pub unseen: bool,
    /// The renderer history slot a deferred frame left for `finish` to resolve into.
    pub deferred_history: Option<usize>,
    /// Remaining frames whose exposure snaps to its meter (capture resets).
    pub settle: u32,
    /// The state holds a host's exposure seed: the next reset adapts from it
    /// instead of snapping (`Renderer::seed_exposure`).
    pub seeded: bool,
    /// Probe-volume GI, when the scene has acquired probe surfaces.
    pub gi: Option<GiGpu>,
    /// Distant shadows past the cascades (`Shadows::far`).
    pub far: Option<super::far::FarMap>,
    /// The cached cascade maps (`Shadows`).
    pub cascades: Option<super::cascades::Cascades>,
    /// Cached local-light shadows (`Quality::local_shadows`).
    pub local: Option<super::local::LocalMap>,
    /// Heightfield horizon shadows and sky visibility (`Quality::horizon`).
    pub horizon: Option<super::horizon::Horizon>,
    /// Virtual shadow maps for the sun (`Quality::virtual_shadows`).
    pub vsm: Option<super::vsm::VirtualShadowMap>,
    /// Idle host staging for probe records, reused.
    pub probe_staging: Vec<super::gi::ProbeStaging>,
    /// Probe records uploaded for the next frame's `GI.Scatter`: staging, count,
    /// and whether they are a pool's finer lattices only (readiness ignores those).
    pub probe_writes: Vec<(super::gi::ProbeStaging, u32, bool)>,
    /// The copied opaque scene transmissive forward draws refract (`look::refract`),
    /// allocated by the first frame that draws one.
    pub refract: Option<super::refract::Refraction>,
    /// Sun shadows of blended casters (`look::translucent`), allocated by the first
    /// High/Epic frame that has one.
    pub translucent: Option<super::translucent::TranslucentSun>,
}

/// Graph identities of the look's persistent resources for one frame.
pub(crate) struct LookIds {
    pub images: [Option<ResourceId>; IMAGES],
    pub state: ResourceId,
    pub frame: ResourceId,
    /// The colour lookup table (host-written between frames).
    pub lut: ResourceId,
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
    /// The horizon's capture, sun and sky visibility (`look::horizon`).
    pub horizon: Option<[ResourceId; 3]>,
    /// The virtual shadow atlas (`look::vsm`).
    pub vsm: Option<ResourceId>,
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
        gpu: &mut GpuDevice,
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
            post_motion: load("look_post_motion")?,
            dof: load("look_dof")?,
            motion: load("look_motion")?,
            gi_relight: load("look_gi_relight")?,
            gi_scatter: load("look_gi_scatter")?,
            ao_trace: load("look_ao_trace")?,
            ao_denoise: load("look_ao_denoise")?,
            ao_bitmask: load("look_ao_bitmask")?,
            ao_bitmask_denoise: load("look_ao_bitmask_denoise")?,
            gi_gather: load("look_gi_gather")?,
            ssr_pyramid: load("look_ssr_pyramid")?,
            ssr_trace: load("look_ssr_trace")?,
            capture_face: load("look_capture_face")?,
            capture_mip: load("look_capture_mip")?,
            capture_prefilter: load("look_capture_prefilter")?,
            capture_refit: load("look_capture_refit")?,
            capture_tiles: load("look_capture_tiles")?,
            horizon_sun: load("look_horizon_sun")?,
            horizon_sky: load("look_horizon_sky")?,
            vsm_mark: load("look_vsm_mark")?,
            vsm_project: load("look_vsm_project")?,
            gi_relight_vsm: load("look_gi_relight_vsm")?,
            refract: load("look_refract")?,
        };
        let frame = gpu.create_buffer(
            std::mem::size_of::<LookFrame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        // Zeroed on the host so the first frame's history readers have a producer.
        let state = gpu.create_buffer(
            STATE_BYTES as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(state, &[0; STATE_BYTES])?;
        let state_index = gpu.storage_buffer(state)?.index;
        let lut = gpu.create_buffer(
            u64::from(LUT_SIZE.pow(3)) * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let lut_index = gpu.storage_buffer(lut)?.index;
        Ok(Self {
            look: Look::default(),
            quality: Quality::default(),
            pipelines,
            frame,
            frame_index,
            state,
            state_index,
            state_access: Access::HostWrite,
            lut,
            lut_index,
            lut_size: 0,
            images: [None; IMAGES],
            sampled: [0; IMAGES],
            storage: [0; IMAGES],
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; IMAGES],
            render: size,
            display: size,
            sky: None,
            deferred: false,
            frame_dof: false,
            frame_motion: false,
            frame_wire: false,
            unseen: false,
            deferred_history: None,
            settle: 0,
            seeded: false,
            gi: None,
            far: None,
            cascades: None,
            local: None,
            horizon: None,
            vsm: None,
            probe_staging: Vec::new(),
            probe_writes: Vec::new(),
            refract: None,
            translucent: None,
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

    /// Drops the view-sized images; the next frame reallocates them.
    pub(crate) fn resize(
        &mut self,
        gpu: &mut GpuDevice,
        render: [u32; 2],
        display: [u32; 2],
    ) -> Result<()> {
        if render == self.render && display == self.display {
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
            VSM_MASK,
            DOF_PREPARED,
            DOF_GATHERED,
            DOF_COMPOSITE,
            MOTION_TILES,
            MOTION_NEIGHBOURS,
        ] {
            if let Some(old) = self.images[i].take() {
                gpu.retire(Resource::Image(old))?;
            }
        }
        self.render = render;
        self.display = display;
        Ok(())
    }

    /// Allocates the images the current look needs (sky LUTs and IBL under a sky,
    /// froxels with sky or fog, bloom atlases with bloom) and the quality budgets
    /// (AO, SSR, depth of field, motion blur: a volume enabling those never allocates
    /// mid-session). Never frees: looks rarely change, and a later frame would only
    /// allocate them again.
    pub(crate) fn ensure(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        ara_gpu::cpu_scope!("Look.ensure");
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
            self.vsm.is_some(),
            self.quality.depth_of_field.is_some(),
            self.quality.depth_of_field.is_some(),
            self.quality.depth_of_field.is_some(),
            self.quality.motion_blur.is_some(),
            self.quality.motion_blur.is_some(),
        ];
        for (i, needed) in needed.into_iter().enumerate() {
            if !needed || self.images[i].is_some() {
                continue;
            }
            let [width, height] = extent(i, self.render, self.display);
            let format = if i == VSM_MASK {
                vk::Format::R16G16_SFLOAT
            } else {
                vk::Format::R16G16B16A16_SFLOAT
            };
            let image = gpu.create_image(
                width,
                height,
                format,
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

    /// Whether ambient occlusion runs as the High/Epic tier's visibility bitmask
    /// (`look-ao.slang aoBitmaskMain`: occlusion plus near-field bounce in the AO
    /// images, which only the tier's lighting variant reads) rather than GTAO.
    pub(crate) fn bitmask_ao(&self) -> bool {
        self.quality.high_tier && self.quality.ambient_occlusion.is_some()
    }

    /// Writes the bitmask pass's rows (`shadowParams[0]`, `shadowInfo[0]`) after the
    /// AO and reflection rows: sample thickness from the AO radius, and whether last
    /// frame's colour pyramid (built this frame with reflections) feeds the bounce.
    pub(crate) fn ambient_rows(&self, frame: &mut LookFrame) {
        if !self.bitmask_ao() {
            return;
        }
        let bounce = frame.flags[1] & super::frame::REFLECTIONS != 0;
        frame.shadow_params[0] = [BITMASK_THICKNESS * frame.ao[0], 1., 0., 0.];
        frame.shadow_info[0] = [u32::from(bounce), 0, 0, 0];
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

    /// Depth of field rows: prepared and gathered (sampled), then (storage).
    pub(crate) fn dof_indices(&self) -> [u32; 4] {
        [
            self.sampled[DOF_PREPARED],
            self.sampled[DOF_GATHERED],
            self.storage[DOF_PREPARED],
            self.storage[DOF_GATHERED],
        ]
    }

    /// Motion blur rows: tile and neighbourhood maxima (sampled), then (storage).
    pub(crate) fn motion_indices(&self) -> [u32; 4] {
        [
            self.sampled[MOTION_TILES],
            self.sampled[MOTION_NEIGHBOURS],
            self.storage[MOTION_TILES],
            self.storage[MOTION_NEIGHBOURS],
        ]
    }

    /// Whether this frame blurs focus and motion: the look asks, the quality budgets
    /// it and its images exist; motion blur also needs the native renderer's motion
    /// vectors (`motion_vectors`). Sets `frame_dof` and `frame_motion`.
    pub(crate) fn blurs(&mut self, motion_vectors: bool) -> (bool, bool) {
        let lens = self.look.lens;
        self.frame_dof = self.quality.depth_of_field.is_some()
            && lens.depth_of_field.is_some()
            && self.images[DOF_COMPOSITE].is_some();
        self.frame_motion = motion_vectors
            && self.quality.motion_blur.is_some()
            && lens.motion_blur > 0.
            && self.images[MOTION_NEIGHBOURS].is_some();
        (self.frame_dof, self.frame_motion)
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
    pub(crate) fn reset_exposure(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.upload_queued(self.state, &[0; STATE_BYTES])?;
        self.state_access = Access::HostWrite;
        self.seeded = false;
        Ok(())
    }

    /// Writes `multiplier` as the applied and previous exposure (EV100 as manual
    /// exposure computes it), with empty metering bins; the next frame adapts from it.
    pub(crate) fn seed_exposure(&mut self, gpu: &mut GpuDevice, multiplier: f32) -> Result<()> {
        let ev100 = (1. / (1.2 * multiplier)).log2();
        let mut bytes = [0u8; STATE_BYTES];
        for (i, value) in [multiplier, multiplier, ev100, 0.].into_iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        gpu.upload_queued(self.state, &bytes)?;
        self.state_access = Access::HostWrite;
        self.seeded = true;
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
            lut: graph.import(
                Resource::Buffer(self.lut),
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
            vsm: self
                .vsm
                .as_ref()
                .map(|v| graph.import(Resource::Image(v.atlas), v.atlas_state)),
            horizon: self.horizon.as_ref().map(|h| {
                let mut import = |image, state| graph.import(Resource::Image(image), state);
                [
                    import(h.depth, h.states[0]),
                    import(h.sun, h.states[1]),
                    import(h.sky, h.states[2]),
                ]
            }),
            probe_writes: self
                .probe_writes
                .iter()
                .map(|(staging, _, _)| {
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
                let state = |access| State {
                    access,
                    queue: Queue::Graphics,
                };
                let (read, write) = (1 - gi.slot, gi.slot);
                [
                    graph.import(Resource::Buffer(gi.cache), state(gi.surface_access[0])),
                    graph.import(Resource::Buffer(gi.meta), state(gi.surface_access[1])),
                    graph.import(Resource::Image(gi.moments), state(gi.surface_access[2])),
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
            for (access, id) in gi.surface_access.iter_mut().zip(&ids[..3]) {
                *access = final_states[id.0].access;
            }
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
        if let (Some(vsm), Some(id)) = (&mut self.vsm, ids.vsm) {
            vsm.atlas_state = final_states[id.0];
        }
        if let (Some(horizon), Some(ids)) = (&mut self.horizon, ids.horizon) {
            for (state, id) in horizon.states.iter_mut().zip(ids) {
                *state = final_states[id.0];
            }
        }
        if let Some(cascades) = &mut self.cascades {
            for (state, id) in cascades.states.iter_mut().zip(&ids.cascades) {
                *state = final_states[id.0];
            }
        }
    }
}
