use crate::{
    probes::{BakedProbes, ProbeVolume},
    scene::{GpuScene, buffer, upload_image},
    settings::*,
    targets::*,
};
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Buffer, Device, Image, Pipeline, Queue, Resource, vk,
    vulkan::{PassTiming, VulkanDevice},
};
use ara_render::graph::State;
use glam::{Mat4, Vec3};
use std::{collections::BTreeMap, path::Path};

/// An offscreen fidelity view. The host owns the device and invokes render at frame
/// boundaries. Resources and temporal histories belong to this view alone.
pub struct FidelityView {
    pub(crate) settings: Settings,
    pub(crate) shader_revision: String,
    pub(crate) scene: GpuScene,
    pub(crate) targets: Targets,
    pub(crate) pipelines: BTreeMap<&'static str, Pipeline>,
    pub(crate) frame_buffer: Buffer,
    pub(crate) frame_index: u32,
    pub(crate) volume: ProbeVolume,
    pub(crate) probes: [Buffer; 2],
    pub(crate) probe_indices: [u32; 2],
    pub(crate) probe_states: [State; 2],
    pub(crate) probe_slot: usize,
    pub(crate) probe_start: u32,
    pub(crate) responsive_frames: u32,
    pub(crate) exposure: Buffer,
    pub(crate) exposure_index: u32,
    pub(crate) exposure_state: State,
    pub(crate) lut: Image,
    pub(crate) lut_index: u32,
    pub(crate) sheen_lut: Image,
    pub(crate) sheen_index: u32,
    pub(crate) previous_vp: Mat4,
    pub(crate) previous_jitter: glam::Vec2,
    pub(crate) previous_camera: Vec3,
    pub(crate) history: bool,
    pub(crate) slot: usize,
    pub(crate) samples: u32,
    pub(crate) gather_events: u32,
    pub(crate) atmosphere: Option<crate::atmosphere::Atmosphere>,
    pub(crate) atmosphere_dirty: bool,
    pub(crate) texture_bias: f32,
    pub(crate) transport_stride: u32,
    pub(crate) timings: Vec<PassTiming>,
    pub(crate) reservoirs: Option<crate::resampling::Reservoirs>,
    pub(crate) shadow_source: crate::SunShadowSource,
}
impl FidelityView {
    /// Upload immutable extraction, prewarm cooked pipelines and allocate view storage.
    /// Raster/Mobile require a bake with matching scene hash; no RT extensions execute.
    pub fn new(
        gpu: &mut VulkanDevice,
        scene: &ara_render::scene::Scene,
        settings: Settings,
        baked: Option<&BakedProbes>,
    ) -> Result<Self> {
        settings.validate()?;
        ensure!(
            settings.tier != Tier::Rt || gpu.context().info.ray_tracing,
            "RT tier unavailable"
        );
        ensure!(
            settings.tier == Tier::Rt || baked.is_some(),
            "Raster/Mobile require baked probes"
        );
        let scene = GpuScene::new(gpu, scene, settings.tier == Tier::Rt)?;
        let volume = if let Some(b) = baked {
            b.validate(scene.content_hash())?;
            b.volume
        } else {
            ProbeVolume::automatic(scene.bounds[0], scene.bounds[1])?
        };
        let empty = volume.empty_data();
        let data = baked.map(|b| b.data.as_slice()).unwrap_or(&empty);
        let probes = [
            buffer(
                gpu,
                bytemuck::cast_slice(data),
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            )?,
            buffer(
                gpu,
                bytemuck::cast_slice(data),
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            )?,
        ];
        let probe_indices = [
            gpu.storage_buffer(probes[0])?.index,
            gpu.storage_buffer(probes[1])?.index,
        ];
        let frame_buffer = gpu.create_buffer(
            std::mem::size_of::<crate::frame::GpuFrame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame_buffer)?.index;
        let exposure = buffer(
            gpu,
            bytemuck::cast_slice(&[1_f32, 0., 0., 0.]),
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
        )?;
        let exposure_index = gpu.storage_buffer(exposure)?.index;
        let mut lut_data = Vec::new();
        for g in 0..16 {
            for b in 0..16 {
                for r in 0..16 {
                    lut_data.push([r as f32 / 15., g as f32 / 15., b as f32 / 15., 1.]);
                }
            }
        }
        let lut = upload_image(
            gpu,
            [256, 16],
            vk::Format::R32G32B32A32_SFLOAT,
            bytemuck::cast_slice(&lut_data),
        )?;
        let lut_index = gpu.sampled_image(lut, false)?.index;
        let sheen_lut = upload_image(
            gpu,
            [32, 32],
            vk::Format::R32G32B32A32_SFLOAT,
            include_bytes!("../shaders/sheen-albedo.bin"),
        )?;
        let sheen_index = gpu.sampled_image(sheen_lut, false)?.index;
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked");
        let mut pipelines = BTreeMap::new();
        let names = if settings.tier == Tier::Rt {
            vec![
                "primary",
                "ddgi",
                "hybrid",
                "reference",
                "primary_emissive",
                "ddgi_emissive",
                "hybrid_emissive",
                "reference_emissive",
                "primary_atmosphere",
                "ddgi_atmosphere",
                "hybrid_atmosphere",
                "reference_atmosphere",
                "primary_layers",
                "ddgi_layers",
                "hybrid_layers",
                "reference_layers",
                "reservoir_init",
                "reservoir_spatial",
                "reservoir_trace",
                "reservoir_init_layers",
                "reservoir_trace_layers",
                "reservoir_spatial_layers",
                "reservoir_init_atmosphere",
                "reservoir_spatial_atmosphere",
                "reservoir_trace_atmosphere",
            ]
        } else {
            vec!["primary_raster", "hybrid_raster"]
        };
        let mut cohort = None;
        for name in names.into_iter().chain([
            "temporal",
            "atrous",
            "temporal_split",
            "atrous_split",
            "bloom",
            "exposure",
            "post",
            "upscale",
            "sharpen",
            "transmittance",
            "multiscatter",
            "skyview",
            "diagnostic",
            if settings.tier == Tier::Rt {
                "fog"
            } else {
                "fog_raster"
            },
        ]) {
            let cooked: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
            if let Some(key) = &cohort {
                ensure!(*key == cooked.key, "Mixed fidelity shader cohorts");
            } else {
                cohort = Some(cooked.key.to_owned());
            }
            pipelines.insert(name, gpu.create_pipeline(&cooked, &artifacts)?);
        }
        let state = State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        };
        Ok(Self {
            settings,
            shader_revision: cohort.expect("Every view loads cooked pipelines"),
            scene,
            targets: Targets::new(
                gpu,
                settings.render_size,
                settings.display_size,
                matches!(settings.mode, Mode::Reference { .. }),
            )?,
            pipelines,
            frame_buffer,
            frame_index,
            volume,
            probes,
            probe_indices,
            probe_states: [state; 2],
            probe_slot: 0,
            probe_start: 0,
            responsive_frames: 0,
            exposure,
            exposure_index,
            exposure_state: state,
            lut,
            lut_index,
            sheen_lut,
            sheen_index,
            previous_vp: Mat4::IDENTITY,
            previous_jitter: glam::Vec2::ZERO,
            previous_camera: Vec3::ZERO,
            history: false,
            slot: 0,
            samples: 0,
            gather_events: 2,
            atmosphere: None,
            atmosphere_dirty: true,
            texture_bias: 0.,
            transport_stride: 2,
            timings: Vec::new(),
            reservoirs: None,
            shadow_source: crate::SunShadowSource::RayQueries,
        })
    }
    /// Bound traced secondary surfaces before the DDGI terminal. One event is
    /// useful with dense large-scene fields; two is the default for small rooms.
    /// Changing this budget invalidates accumulated transport at a frame boundary.
    pub fn set_gather_events(&mut self, events: u32) -> Result<()> {
        ensure!(
            (1..=4).contains(&events),
            "Gather events must be between one and four"
        );
        if self.gather_events != events {
            self.gather_events = events;
            self.history = false;
        }
        Ok(())
    }
    /// Trace one incident sample per two or four pixels each frame. Rotating phases
    /// cover every pixel; invalid history always receives a fresh repair sample.
    /// Direct lighting and primary visibility remain full rate. Changing the rate resets history.
    pub fn set_transport_stride(&mut self, stride: u32) -> Result<()> {
        ensure!(
            matches!(stride, 2 | 4),
            "Transport stride must be two or four"
        );
        if self.transport_stride != stride {
            self.transport_stride = stride;
            self.history = false;
        }
        Ok(())
    }
    /// Apply an extracted scene update between completed frames. Changed geometry
    /// retains previous positions for one frame; changed lighting should set the
    /// FrameInput lighting_changed flag to reject accumulated radiance.
    pub fn update_scene(
        &mut self,
        gpu: &mut VulkanDevice,
        scene: &ara_render::scene::Scene,
    ) -> Result<()> {
        self.scene.update(gpu, scene)
    }
    /// Change ray-query visibility while retaining cached mesh acceleration.
    pub fn set_instance_masks(
        &mut self,
        gpu: &mut VulkanDevice,
        masks: &[(u64, u8)],
    ) -> Result<()> {
        self.scene.set_instance_masks(gpu, masks)?;
        self.history = false;
        Ok(())
    }
    /// Native offscreen output, in CopySource layout after each completed render.
    pub fn output(&self) -> Image {
        self.targets.images[OUTPUT]
    }
    /// Export the common temporal input bundle to an external reconstruction backend.
    /// Linear transport uses pre-exposure 1; display exposure is a later post operation.
    /// The host supplies reset when assigning a viewport or changing its camera/content.
    pub fn upscaler_input(&self, reset: bool) -> UpscalerInput {
        UpscalerInput {
            colour: self.targets.images[FILTER0],
            depth: self.targets.images[DEPTH],
            motion: self.targets.images[MOTION],
            jitter: self.previous_jitter.to_array(),
            exposure: 1.,
            reset,
            render_size: self.settings.render_size,
            display_size: self.settings.display_size,
        }
    }
    /// Validated allocation/settings contract.
    pub fn settings(&self) -> Settings {
        self.settings
    }
    /// Replace only offscreen view targets; cached scene/AS/probes remain resident.
    /// New storage is allocated before old images retire, and every history resets.
    pub fn resize(
        &mut self,
        gpu: &mut VulkanDevice,
        render_size: [u32; 2],
        display_size: [u32; 2],
    ) -> Result<()> {
        let mut settings = self.settings;
        settings.render_size = render_size;
        settings.display_size = display_size;
        settings.validate()?;
        if settings.render_size == self.settings.render_size
            && settings.display_size == self.settings.display_size
        {
            return Ok(());
        }
        let mut replacement = Targets::new(
            gpu,
            render_size,
            display_size,
            matches!(self.settings.mode, Mode::Reference { .. }),
        )?;
        if self.reservoirs.is_some() {
            replacement.enable_split(gpu, render_size)?;
        }
        std::mem::replace(&mut self.targets, replacement).destroy(gpu)?;
        if let Some(old) = self.reservoirs.take() {
            self.reservoirs = Some(crate::resampling::Reservoirs::new(gpu, render_size)?);
            old.destroy(gpu)?;
        }
        self.settings = settings;
        self.shadow_source = crate::SunShadowSource::RayQueries;
        self.history = false;
        self.samples = 0;
        gpu.collect()?;
        Ok(())
    }
    /// Driver timestamp results; capture transfer is measured separately.
    pub fn timings(&self) -> &[PassTiming] {
        &self.timings
    }
    /// Number of independent accumulated reference samples per pixel.
    pub fn reference_samples(&self) -> u32 {
        self.samples
    }
    /// Read-only GPU extraction identity and acceleration-update diagnostics.
    pub fn scene(&self) -> &GpuScene {
        &self.scene
    }
    /// Cooked shader cohort used to reject stale cached ground truth.
    pub fn shader_revision(&self) -> &str {
        &self.shader_revision
    }
    /// Content hash for reference, bake and capture provenance.
    pub fn scene_hash(&self) -> &str {
        self.scene.content_hash()
    }
    /// Change physical post controls without reallocating view resources.
    pub fn set_post(&mut self, post: PostSettings) -> Result<()> {
        let mut s = self.settings;
        s.post = post;
        s.validate()?;
        self.settings = s;
        Ok(())
    }
    /// Set a linear 16x16x16 LUT, packed as rows G, columns B*16+R.
    pub fn set_lut(&mut self, gpu: &mut VulkanDevice, data: &[[f32; 4]]) -> Result<()> {
        ensure!(
            data.len() == 4096 && data.iter().flatten().all(|v| v.is_finite()),
            "Invalid grading LUT"
        );
        let lut = upload_image(
            gpu,
            [256, 16],
            vk::Format::R32G32B32A32_SFLOAT,
            bytemuck::cast_slice(data),
        )?;
        let index = gpu.sampled_image(lut, false)?.index;
        gpu.retire(Resource::Image(std::mem::replace(&mut self.lut, lut)))?;
        self.lut_index = index;
        gpu.collect()?;
        Ok(())
    }
    /// Explicit GPU readback for offline baking; normal frames never map probe data.
    pub fn bake_snapshot(&mut self, gpu: &mut VulkanDevice) -> Result<BakedProbes> {
        let index = 1 - self.probe_slot;
        let bytes = self.read_buffer(
            gpu,
            self.probes[index],
            self.volume.len() * crate::probes::PROBE_VECTORS * 16,
            self.probe_states[index],
        )?;
        self.probe_states[index] = State {
            access: Access::CopySource,
            queue: Queue::Graphics,
        };
        let data = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|v| {
                std::array::from_fn(|i| {
                    f32::from_le_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]])
                })
            })
            .collect();
        Ok(BakedProbes {
            version: 1,
            volume: self.volume,
            scene_hash: self.scene.hash.to_owned(),
            data,
        })
    }
    fn read_buffer(
        &self,
        gpu: &mut VulkanDevice,
        source: Buffer,
        bytes: usize,
        state: State,
    ) -> Result<Vec<u8>> {
        use ara_gpu::{Barrier, vulkan::Command};
        let b = gpu.create_buffer(bytes as u64, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let mut c = gpu.begin(Queue::Graphics);
        c.barriers([Barrier {
            resource: Resource::Buffer(source),
            before: state.access,
            after: Access::CopySource,
            handoff: None,
        }]);
        c.push(Command::CopyBuffer {
            source,
            destination: b,
            size: bytes as u64,
        });
        c.barriers([Barrier {
            resource: Resource::Buffer(b),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let p = gpu.submit(c.finish(), &[])?;
        gpu.resolve(p)?;
        let data = gpu.read(b)?;
        gpu.retire(Resource::Buffer(b))?;
        gpu.collect()?;
        Ok(data)
    }
    /// Release view-owned resources after its last completed frame.
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some(r) = self.reservoirs {
            r.destroy(gpu)?;
        }
        self.targets.destroy(gpu)?;
        self.scene.destroy(gpu)?;
        for b in self
            .probes
            .into_iter()
            .chain([self.frame_buffer, self.exposure])
        {
            gpu.retire(Resource::Buffer(b))?;
        }
        gpu.retire(Resource::Image(self.lut))?;
        gpu.retire(Resource::Image(self.sheen_lut))?;
        for (_, p) in self.pipelines {
            gpu.retire_pipeline(p)?;
        }
        gpu.collect()?;
        Ok(())
    }
}
