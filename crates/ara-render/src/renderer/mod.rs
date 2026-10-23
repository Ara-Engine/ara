//! Renderer orchestration. Extraction snapshots are uploaded once; frames record handles.
mod frame;
mod passes;
pub mod post;
mod transfer;
mod upload;
use crate::{
    capture::{Capture, CaptureRequest},
    graph::{Graph, State, TransientImage},
    scene::{CameraSpec, Scene},
};
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Buffer, Device, Image, Pipeline, Queue, Resource, vk,
    vulkan::{PassTiming, VulkanDevice},
};
use glam::{Mat4, Vec3};
use std::path::{Path, PathBuf};

const ALBEDO: usize = 0;
const NORMAL: usize = 1;
const MATERIAL: usize = 2;
const WORLD: usize = 3;
const ID: usize = 4;
const DEPTH: usize = 5;
const SHADOW: usize = 6;
const HDR: usize = 7;
const HISTORY0: usize = 8;
const HISTORY1: usize = 9;
const OUTPUT: usize = 10;
const TARGET_COUNT: usize = 11;

struct Mesh {
    vertices: Buffer,
    indices: Buffer,
    count: u32,
    material: u32,
    transparent: bool,
    centre: Vec3,
}
struct Targets {
    images: Vec<Image>,
    sampled: Vec<u32>,
    storage: [u32; 3],
    states: Vec<State>,
    tiles: Buffer,
    tile_index: u32,
    read_color: Buffer,
    read_ids: Buffer,
    history: Option<usize>,
}
struct Pipelines {
    shadow: Pipeline,
    gbuffer: Pipeline,
    tile: Pipeline,
    lighting: Pipeline,
    forward: Pipeline,
    taa: Pipeline,
    tonemap: Pipeline,
}

/// Vulkan renderer with persistent extracted scene resources and per-view histories.
/// Calls are serialized for v0 diagnostics; logical queue transfers remain explicit.
pub struct Renderer {
    /// GPU ownership and native window integration surface.
    gpu: VulkanDevice,
    size: [u32; 2],
    targets: Targets,
    pipelines: Pipelines,
    meshes: Vec<Mesh>,
    graphics_inputs: Vec<(Resource, Access)>,
    compute_inputs: Vec<(Resource, Access)>,
    material_index: u32,
    frame: Buffer,
    frame_index: u32,
    compute_frame: Buffer,
    compute_frame_index: u32,
    light_index: u32,
    compute_light_index: u32,
    light_count: u32,
    environment: u32,
    irradiance: u32,
    prefilter: u32,
    sun_direction: Vec3,
    sun_lux: Vec3,
    previous_vp: Mat4,
    frame_number: u32,
    scene_hash: String,
    shader_metadata: crate::capture::ShaderMetadata,
    entity_names: Vec<(u64, String)>,
    last_timings: Vec<PassTiming>,
}

impl Renderer {
    /// Read-only GPU diagnostics and native presentation integration.
    pub fn gpu(&self) -> &VulkanDevice {
        &self.gpu
    }
    /// Borrow the device between completed frames to record independent graph extensions.
    /// Extensions own their targets and must preserve this renderer's resource states.
    pub fn gpu_mut(&mut self) -> &mut VulkanDevice {
        &mut self.gpu
    }
    /// Upload an extraction snapshot and prewarm its complete cooked pipeline set.
    /// Shader cooking must have happened offline, before this call.
    pub fn new(
        scene: Scene,
        size: [u32; 2],
        extensions: &[std::ffi::CString],
        cache: impl Into<PathBuf>,
    ) -> Result<Self> {
        ensure!(!size.contains(&0), "Render extent is zero");
        let mut gpu = VulkanDevice::new(extensions, cache)?;
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked");
        let metadata: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(artifacts.join("gbuffer.json"))?)?;
        let shader_metadata = crate::capture::ShaderMetadata {
            content_key: metadata.key,
            compiler: metadata.compiler,
            binding_layout_hash: metadata.binding_layout_hash,
        };
        let mut load = |name: &str| -> Result<_> {
            let cooked: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
            ensure!(
                cooked.key == shader_metadata.content_key,
                "Baseline cook is incomplete; recook the pipeline manifest"
            );
            Ok(gpu.create_pipeline(&cooked, &artifacts)?)
        };
        let pipelines = Pipelines {
            shadow: load("shadow")?,
            gbuffer: load("gbuffer")?,
            tile: load("tile")?,
            lighting: load("lighting")?,
            forward: load("forward")?,
            taa: load("taa")?,
            tonemap: load("tonemap")?,
        };
        let targets = Self::targets(&mut gpu, size)?;
        Self::upload_scene(
            gpu,
            pipelines,
            targets,
            scene,
            size,
            &artifacts,
            shader_metadata,
        )
    }
    /// Native frame size; resize replaces history and retires old graph storage.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// Current SDR output, in CopySource layout after rendering.
    pub fn output(&self) -> Image {
        self.targets.images[OUTPUT]
    }
    /// Last measured pass timings, including actual dedicated-queue execution.
    pub fn timings(&self) -> &[PassTiming] {
        &self.last_timings
    }
    /// Reallocate per-view graph resources and reset temporal history.
    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        ensure!(!size.contains(&0), "Cannot render a minimized window");
        if size == self.size {
            return Ok(());
        }
        let replacement = Self::targets(&mut self.gpu, size)?;
        let previous = std::mem::replace(&mut self.targets, replacement);
        for image in previous.images {
            self.gpu.retire(Resource::Image(image))?;
        }
        for buffer in [previous.tiles, previous.read_color, previous.read_ids] {
            self.gpu.retire(Resource::Buffer(buffer))?;
        }
        self.gpu.collect()?;
        self.size = size;
        self.frame_number = 0;
        Ok(())
    }
    /// Render live with TAA jitter and camera reprojection at a locked physical exposure.
    pub fn render(&mut self, camera: &CameraSpec, exposure: f32) -> Result<()> {
        self.render_frame(camera, exposure, false, crate::capture::ViewMode::Lit)?;
        Ok(())
    }
    /// Render an agent request, with reset history and an explicit settle frame count.
    pub fn capture(&mut self, request: &CaptureRequest) -> Result<Capture> {
        self.resize(request.size)?;
        self.targets.history = None;
        self.frame_number = 0;
        ensure!(request.settle_frames <= 1024, "Settle frame limit is 1024");
        for _ in 0..request.settle_frames.max(1) {
            self.render_frame(
                &request.camera,
                request.exposure,
                request.deterministic,
                request.mode,
            )?;
        }
        let rgba = self.gpu.read(self.targets.read_color)?;
        let ids = self
            .gpu
            .read(self.targets.read_ids)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u32)
            .collect::<Vec<_>>();
        Capture::from_render(
            rgba,
            ids,
            request,
            crate::capture::CaptureContext {
                names: &self.entity_names,
                hash: &self.scene_hash,
                device: &self.gpu.context().info,
                timings: &self.last_timings,
                shaders: &self.shader_metadata,
            },
        )
    }
    fn targets(gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<Targets> {
        let mut graph = Graph::default();
        let mut images = Vec::new();
        let mut sampled = Vec::new();
        for target in 0..TARGET_COUNT {
            let format = match target {
                ID => vk::Format::R32_SFLOAT,
                DEPTH | SHADOW => vk::Format::D32_SFLOAT,
                OUTPUT => vk::Format::R8G8B8A8_UNORM,
                _ => vk::Format::R32G32B32A32_SFLOAT,
            };
            let depth = target == DEPTH || target == SHADOW;
            let usage = vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | if depth {
                    vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                } else if target >= HISTORY0 {
                    vk::ImageUsageFlags::STORAGE
                } else {
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                };
            let extent = if target == SHADOW { [1024, 1024] } else { size };
            let (_, image) = graph.transient(
                gpu,
                TransientImage {
                    size: extent,
                    format,
                    usage,
                },
                Queue::Graphics,
            )?;
            images.push(image);
            sampled.push(gpu.sampled_image(image, depth)?.index);
        }
        let storage = [
            gpu.storage_image(images[HISTORY0])?.index,
            gpu.storage_image(images[HISTORY1])?.index,
            gpu.storage_image(images[OUTPUT])?.index,
        ];
        let tiles = gpu.create_buffer(
            u64::from(size[0].div_ceil(16)) * u64::from(size[1].div_ceil(16)) * 65 * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            false,
        )?;
        let tile_index = gpu.storage_buffer(tiles)?.index;
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        let read_color = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let read_ids = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        Ok(Targets {
            images,
            sampled,
            storage,
            states: vec![
                State {
                    access: Access::Undefined,
                    queue: Queue::Graphics
                };
                TARGET_COUNT
            ],
            tiles,
            tile_index,
            read_color,
            read_ids,
            history: None,
        })
    }
}
