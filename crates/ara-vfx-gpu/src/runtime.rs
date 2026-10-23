use crate::{COMPUTE_ENTRIES, layout::*, stats::*};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Device, Image, Pipeline, Queue, Resource, vk, vulkan::VulkanDevice};
use ara_render::{
    capture::{Capture, CaptureRequest, FeatureCapture, ShaderMetadata},
    graph::State,
    scene::CameraSpec,
};
use ara_vfx::{Effect, Module};
use bytemuck::Zeroable;
use glam::Vec3;
use std::path::Path;

/// Per-view extracted data; one instance may render into several views sequentially.
pub struct View {
    /// Camera using Ara's reversed-Z projection.
    pub camera: CameraSpec,
    /// Positive linear exposure before tone mapping.
    pub exposure: f32,
    /// Use a dedicated compute family. Enable only after measuring this workload.
    pub async_compute: bool,
    /// Optional same-sized scene depth. Graphics ownership is returned as ShaderRead.
    pub scene_depth: Option<SceneDepth>,
    /// Optional HDR scene color to composite into; requires scene_depth.
    pub scene_color: Option<SceneColor>,
}
/// Imported linear HDR scene color, preserved beneath the VFX draw.
#[derive(Clone, Copy)]
pub struct SceneColor {
    /// Same-device RGBA32-float attachment matching the feature's size.
    pub image: Image,
    /// Published sampled-image index for tone mapping the composited result.
    pub sampled_index: u32,
    /// Actual prior access, owned by Graphics. Returns in Graphics/ShaderRead.
    pub state: State,
}
/// Imported opaque scene depth, shared by soft particles and GPU collisions.
#[derive(Clone, Copy)]
pub struct SceneDepth {
    /// Same-device sampled image containing reversed-Z depth.
    pub image: Image,
    /// Its published native sampled-image heap index.
    pub sampled_index: u32,
    /// Actual prior access; ownership must already belong to Graphics.
    pub state: State,
}
impl Default for View {
    fn default() -> Self {
        Self {
            camera: CameraSpec::LookAt {
                position: Vec3::new(5., 3.4, 6.),
                target: Vec3::new(0., 1.4, 0.),
                vertical_fov: 0.75,
            },
            exposure: 1.,
            async_compute: false,
            scene_depth: None,
            scene_color: None,
        }
    }
}
pub(crate) struct Tracked {
    pub resource: Resource,
    pub state: State,
}
pub(crate) struct Pipelines {
    pub compute: Vec<Pipeline>,
    pub backdrop: Pipeline,
    pub particles: Pipeline,
    pub tonemap: Pipeline,
}
/// Owned GPU particle resources. The engine retains the device; this can be an ECS
/// render component and registers passes without changing ara-render's scene type.
pub struct GpuSystem {
    pub(crate) effect: Effect,
    pub(crate) layout: Layout,
    pub(crate) resources: Vec<Tracked>,
    pub(crate) particles: Buffer,
    pub(crate) lists: Buffer,
    pub(crate) control: Buffer,
    pub(crate) frame: Buffer,
    pub(crate) read_control: Buffer,
    pub(crate) read_color: Buffer,
    pub(crate) hdr: Image,
    pub(crate) depth: Image,
    pub(crate) output: Image,
    pub(crate) feature_ids: Image,
    pub(crate) read_ids: Buffer,
    pub(crate) material_buffer: Buffer,
    pub(crate) material_index: u32,
    pub(crate) material_textures: Vec<(Image, State)>,
    pub(crate) indices: [u32; 4],
    pub(crate) config_index: u32,
    pub(crate) depth_index: u32,
    pub(crate) hdr_index: u32,
    pub(crate) color_target: Image,
    pub(crate) color_index: u32,
    pub(crate) preview_background: bool,
    pub(crate) pipelines: Pipelines,
    pub(crate) size: [u32; 2],
    pub(crate) tick: u32,
    pub(crate) timings: Vec<ara_gpu::commands::PassTiming>,
    pub(crate) metadata: ShaderMetadata,
    pub(crate) hash: String,
    pub(crate) visible: u32,
    pub(crate) async_compute: bool,
    pub(crate) last_frame: Frame,
}
impl GpuSystem {
    /// Load matching offline pipelines and allocate device-local SoA/list storage.
    /// No shader compiler runs here. Shader/pipeline publication belongs at a frame boundary.
    pub fn new(
        gpu: &mut VulkanDevice,
        effect: &Effect,
        cooked: &Path,
        size: [u32; 2],
    ) -> Result<Self> {
        ensure!(!size.contains(&0), "zero VFX capture extent");
        let hash = effect
            .content_hash()
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;
        let effect = effect
            .resolve(&Default::default())
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;
        let stored: serde_json::Value =
            ara_json::parse(&std::fs::read_to_string(cooked.join("effect.json"))?)?;
        ensure!(
            stored.get("hash").and_then(|h| h.as_str()) == Some(hash.as_str()),
            "cooked VFX effect hash differs; recook in a worker before publication"
        );
        let mut cohort = None;
        let mut load = |name: &str| -> Result<Pipeline> {
            let p: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
            if let Some(ref old) = cohort {
                ensure!(*old == p.key, "mixed VFX shader cohort");
            } else {
                cohort = Some(p.key.clone());
            }
            Ok(gpu.create_pipeline(&p, cooked)?)
        };
        let compute = COMPUTE_ENTRIES
            .iter()
            .map(|name| load(name))
            .collect::<Result<Vec<_>>>()?;
        let pipelines = Pipelines {
            compute,
            backdrop: load("backdrop")?,
            particles: load("particles")?,
            tonemap: load("tonemap")?,
        };
        let p: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join("particles.json"))?)?;
        let metadata = ShaderMetadata {
            content_key: p.key,
            compiler: p.compiler,
            binding_layout_hash: p.binding_layout_hash,
        };
        let layout = Layout::new(&effect)?;
        let mut resources = Vec::new();
        let mut buffer = |gpu: &mut VulkanDevice,
                          size: u64,
                          host: bool,
                          usage: vk::BufferUsageFlags|
         -> Result<Buffer> {
            let buffer = gpu.create_buffer(size, usage, host)?;
            resources.push(Tracked {
                resource: Resource::Buffer(buffer),
                state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
            });
            Ok(buffer)
        };
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::TRANSFER_SRC;
        let particles = buffer(gpu, u64::from(layout.total) * 64, false, storage)?;
        let lists = buffer(gpu, u64::from(layout.list_bytes), false, storage)?;
        let control = buffer(
            gpu,
            layout.configs.len() as u64 * CONTROL_STRIDE as u64,
            false,
            storage | vk::BufferUsageFlags::INDIRECT_BUFFER,
        )?;
        let config = buffer(
            gpu,
            layout.configs.len() as u64 * CONFIG_STRIDE as u64,
            true,
            storage,
        )?;
        let frame = buffer(gpu, std::mem::size_of::<Frame>() as u64, true, storage)?;
        let read_control = buffer(
            gpu,
            layout.configs.len() as u64 * CONTROL_STRIDE as u64,
            true,
            vk::BufferUsageFlags::TRANSFER_DST,
        )?;
        let read_color = buffer(
            gpu,
            u64::from(size[0]) * u64::from(size[1]) * 4,
            true,
            vk::BufferUsageFlags::TRANSFER_DST,
        )?;
        let mut image = |format, usage| -> Result<Image> {
            let i = gpu.create_image(size[0], size[1], format, usage)?;
            resources.push(Tracked {
                resource: Resource::Image(i),
                state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
            });
            Ok(i)
        };
        let attachment = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let hdr = image(vk::Format::R32G32B32A32_SFLOAT, attachment)?;
        let depth = image(vk::Format::R32_SFLOAT, attachment)?;
        let output = image(
            vk::Format::R8G8B8A8_UNORM,
            attachment | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let feature_ids = image(
            vk::Format::R32_SFLOAT,
            attachment | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let read_ids = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        resources.push(Tracked {
            resource: Resource::Buffer(read_ids),
            state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
        });
        let material_buffer =
            gpu.create_buffer(16 * 64 * 16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.upload(material_buffer, &vec![0u8; 16 * 64 * 16])?;
        resources.push(Tracked {
            resource: Resource::Buffer(material_buffer),
            state: State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        });
        let material_index = gpu.storage_buffer(material_buffer)?.index;
        let indices = [
            gpu.storage_buffer(particles)?.index,
            gpu.storage_buffer(lists)?.index,
            gpu.storage_buffer(control)?.index,
            gpu.storage_buffer(frame)?.index,
        ];
        let config_index = gpu.storage_buffer(config)?.index;
        let depth_index = gpu.sampled_image(depth, false)?.index;
        let hdr_index = gpu.sampled_image(hdr, false)?.index;
        gpu.upload(config, bytemuck::cast_slice(&layout.configs))?;
        resources[3].state.access = Access::HostWrite;
        let mut result = Self {
            effect,
            layout,
            resources,
            particles,
            lists,
            control,
            frame,
            read_control,
            read_color,
            hdr,
            depth,
            output,
            feature_ids,
            read_ids,
            material_buffer,
            material_index,
            material_textures: Vec::new(),
            indices,
            config_index,
            depth_index,
            hdr_index,
            color_target: hdr,
            color_index: hdr_index,
            preview_background: true,
            pipelines,
            size,
            tick: 0,
            timings: vec![],
            metadata,
            hash,
            visible: 0,
            async_compute: false,
            last_frame: Frame::zeroed(),
        };
        result.reset(gpu)?;
        Ok(result)
    }
    /// Reset deterministic replay state without changing compiled programs.
    pub fn reset(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        let mut particle_data = vec![0u32; self.layout.total as usize * 16];
        for slot in 0..self.layout.total as usize {
            particle_data[slot * 4 + 3] = (-1f32).to_bits();
        }
        let mut lists = vec![0u32; self.layout.list_bytes as usize / 4];
        let mut control = vec![0u32; self.layout.configs.len() * 16];
        for (i, c) in self.layout.configs.iter().enumerate() {
            for slot in 0..c.partition[1] {
                lists[(c.lists[1] / 4 + slot) as usize] = slot;
            }
            control[i * 16 + 1] = c.partition[1];
            control[i * 16 + 8] = c.partition[1].div_ceil(256);
            control[i * 16 + 9] = 1;
            control[i * 16 + 10] = 1;
        }
        self.initialize_buffer(gpu, 0, bytemuck::cast_slice(&particle_data))?;
        self.initialize_buffer(gpu, 1, bytemuck::cast_slice(&lists))?;
        self.initialize_buffer(gpu, 2, bytemuck::cast_slice(&control))?;
        self.tick = 0;
        self.timings.clear();
        Ok(())
    }
    fn initialize_buffer(
        &mut self,
        gpu: &mut VulkanDevice,
        index: usize,
        bytes: &[u8],
    ) -> Result<()> {
        use ara_gpu::{Barrier, commands::Command};
        let Resource::Buffer(destination) = self.resources[index].resource else {
            anyhow::bail!("initialization expects buffer");
        };
        let staging =
            gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        gpu.upload(staging, bytes)?;
        let state = self.resources[index].state;
        let mut r = gpu.begin(state.queue);
        r.push(Command::Barrier(Barrier {
            resource: Resource::Buffer(staging),
            before: Access::HostWrite,
            after: Access::CopySource,
            handoff: None,
        }));
        r.push(Command::Barrier(Barrier {
            resource: Resource::Buffer(destination),
            before: state.access,
            after: Access::CopyDestination,
            handoff: None,
        }));
        r.push(Command::CopyBuffer {
            source: staging,
            destination,
            size: bytes.len() as u64,
        });
        let point = gpu.submit(r.finish(), &[])?;
        gpu.resolve(point)?;
        gpu.retire(Resource::Buffer(staging))?;
        gpu.collect()?;
        self.resources[index].state.access = Access::CopyDestination;
        Ok(())
    }
    /// Advance one fixed tick and render it through declared graph passes.
    pub fn step(&mut self, gpu: &mut VulkanDevice, view: &View) -> Result<()> {
        self.run_frame(gpu, view, false, true)
    }
    /// Replay from tick zero and assemble a standard ara-render capture at time T.
    /// Capture tick is completed fixed ticks; settle frames are bounded and do not advance time.
    pub fn capture(&mut self, gpu: &mut VulkanDevice, request: &CaptureRequest) -> Result<Capture> {
        ensure!(
            request.deterministic && request.seed == u64::from(self.effect.seed),
            "VFX captures require deterministic mode and the authored seed"
        );
        ensure!(
            matches!(request.mode, ara_render::capture::ViewMode::Lit),
            "VFX feature capture currently supports the lit channel"
        );
        ensure!(
            request.size == self.size
                && request.tick > 0
                && request.tick <= 1_000_000
                && request.settle_frames <= 1024,
            "invalid VFX capture extent, tick or settle count"
        );
        self.reset(gpu)?;
        let view = View {
            camera: request.camera.clone(),
            exposure: request.exposure,
            async_compute: false,
            scene_depth: None,
            scene_color: None,
        };
        for _ in 1..request.tick {
            self.run_frame(gpu, &view, false, true)?;
        }
        let settle = request.settle_frames.max(1);
        self.run_frame(gpu, &view, settle == 1, true)?;
        for frame in 1..settle {
            self.run_frame(gpu, &view, frame + 1 == settle, false)?;
        }
        let rgba = gpu.read(self.read_color)?;
        let ids = gpu
            .read(self.read_ids)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| f32::from_le_bytes([p[0], p[1], p[2], p[3]]) as u32)
            .collect();
        let names = vec![(1, self.effect.name.clone())];
        Capture::from_feature(
            FeatureCapture {
                rgba,
                ids,
                names: &names,
                hash: &self.hash,
                device: &gpu.context().info,
                timings: &self.timings,
                shaders: &self.metadata,
            },
            request,
        )
    }
    /// Read GPU counters and quality diagnostics after the most recent completed frame.
    pub fn stats(&self, gpu: &VulkanDevice) -> Result<Stats> {
        ensure!(self.tick > 0, "step before querying GPU stats");
        let bytes = gpu.read(self.read_control)?;
        let words = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect::<Vec<_>>();
        let emitters = self
            .effect
            .emitters
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let w = &words[i * 16..];
                EmitterStats {
                    name: e.name.clone(),
                    alive: w[0],
                    dead: w[1],
                    rendered: w[14],
                    events: w[2],
                    dropped_spawns: w[12],
                    dropped_events: w[13],
                }
            })
            .collect::<Vec<_>>();
        let overdraw_estimate = emitters
            .iter()
            .enumerate()
            .map(|(i, _)| f32::from_bits(words[i * 16 + 15]))
            .sum();
        let mut diagnostics = self
            .effect
            .diagnostics()
            .iter()
            .map(|d| format!("{d:?}"))
            .collect::<Vec<_>>();
        for e in &emitters {
            if e.dropped_spawns > 0 {
                diagnostics.push(format!(
                    "{}: {} spawns dropped at capacity",
                    e.name, e.dropped_spawns
                ));
            }
            if e.dropped_events > 0 {
                diagnostics.push(format!(
                    "{}: {} events/children dropped by quality cap",
                    e.name, e.dropped_events
                ));
            }
            if e.rendered < e.alive {
                diagnostics.push(format!(
                    "{}: render instances limited by culling/overdraw",
                    e.name
                ));
            }
        }
        Ok(Stats {
            tick: self.tick,
            particles_alive: emitters.iter().map(|e| e.alive).sum(),
            emitters,
            gpu_stages: self.timings.to_vec(),
            overdraw_estimate,
            batches: self.visible,
            lights: self
                .layout
                .configs
                .iter()
                .filter(|c| c.light_color[3] > 0.)
                .count() as u32,
            device: serde_json::to_value(&gpu.context().info)?,
            effect_hash: self.hash.clone(),
            async_compute: self.async_compute,
            diagnostics,
        })
    }
    pub(crate) fn mesh_diameter(&self, index: usize) -> f32 {
        self.effect.emitters[index]
            .stacks
            .render
            .iter()
            .fold(1., |max, m| match &m.module {
                Module::Mesh { triangles, .. } => triangles
                    .iter()
                    .flatten()
                    .fold(max, |r, v| r.max(2. * Vec3::from(*v).length())),
                _ => max,
            })
    }
    /// Current SDR output image; ownership/layout remain tracked by this feature.
    pub fn output(&self) -> Image {
        self.output
    }
    /// Bind material-system instance rows and all texture images reachable by them.
    /// Call at a completed frame boundary; textures return in Graphics/ShaderRead.
    /// Packed rows must use the layouts of the materials supplied to cook_source.
    pub fn bind_materials(
        &mut self,
        gpu: &mut VulkanDevice,
        parameters: &[ara_material::PackedParameters],
        textures: &[(Image, State)],
    ) -> Result<()> {
        ensure!(
            parameters.len() == self.effect.emitters.len(),
            "one packed material per emitter"
        );
        ensure!(
            textures.iter().all(|(_, s)| s.queue == Queue::Graphics),
            "material textures must be Graphics-owned"
        );
        let mut rows = vec![[0f32; 4]; 16 * 64];
        for (i, p) in parameters.iter().enumerate() {
            ensure!(
                p.rows.len() <= 64 && p.rows.iter().flatten().all(|v| v.is_finite()),
                "invalid material rows"
            );
            rows[i * 64..i * 64 + p.rows.len()].copy_from_slice(&p.rows);
        }
        gpu.upload(self.material_buffer, bytemuck::cast_slice(&rows))?;
        self.resources[12].state.access = Access::HostWrite;
        self.material_textures = textures.to_vec();
        Ok(())
    }
    /// Explicitly retire feature-owned resources after their submitted work completes.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        for r in self.resources {
            gpu.retire(r.resource)?;
        }
        for pipeline in self.pipelines.compute.into_iter().chain([
            self.pipelines.backdrop,
            self.pipelines.particles,
            self.pipelines.tonemap,
        ]) {
            gpu.retire_pipeline(pipeline)?;
        }
        gpu.collect()?;
        Ok(())
    }
}
