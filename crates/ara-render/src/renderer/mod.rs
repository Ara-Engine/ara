//! Renderer orchestration. Extraction snapshots are uploaded once; frames record handles.
mod edit;
mod frame;
mod gi;
mod hash;
mod native;
mod passes;
pub mod post;
mod readback;
mod submission;
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
const HDR: usize = 6;
const HISTORY0: usize = 7;
const HISTORY1: usize = 8;
const OUTPUT: usize = 9;
const MOTION: usize = 10;
const TARGET_COUNT: usize = 11;

struct Mesh {
    /// Stable external identity, the key for `Renderer::update_mesh`.
    entity: u64,
    vertices: Buffer,
    indices: Buffer,
    count: u32,
    vertex_count: usize,
    material: u32,
    transparent: bool,
    centre: Vec3,
    /// Vertices live in host-visible storage rewritten between frames.
    host: bool,
    /// Last dynamic upload, retained for capture provenance hashing.
    latest: Option<Vec<crate::scene::Vertex>>,
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
    content: upload::Content,
    frame: Buffer,
    frame_index: u32,
    compute_frame: Buffer,
    compute_frame_index: u32,
    previous_vp: Mat4,
    frame_number: u32,
    artifacts: PathBuf,
    shader_metadata: crate::capture::ShaderMetadata,
    last_timings: Vec<PassTiming>,
    /// Sky, fog, exposure, bloom and display grading after the G-buffer.
    look: crate::look::gpu::LookGpu,
    native: Option<native::Native>,
}

impl Renderer {
    /// Read-only GPU diagnostics and native presentation integration.
    pub fn gpu(&self) -> &VulkanDevice {
        &self.gpu
    }
    /// Opaque reversed-Z depth, sampled index and actual state for feature graphs.
    /// Feature consumers must return it to Graphics/ShaderRead before the next frame.
    pub fn feature_depth(&self) -> (Image, u32, State) {
        (
            self.targets.images[DEPTH],
            self.targets.sampled[DEPTH],
            self.targets.states[DEPTH],
        )
    }
    /// Linear HDR scene color for transparent graph features after a completed frame.
    /// Consumers must preserve content and return it to Graphics/ShaderRead.
    pub fn feature_color(&self) -> (Image, u32, State) {
        (
            self.targets.images[HDR],
            self.targets.sampled[HDR],
            self.targets.states[HDR],
        )
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
        Self::new_device(scene, size, extensions, cache.into(), false)
    }
    /// Raster-only bootstrap on every GPU. No ray-query/AS features or heap are
    /// enabled; native content can be adopted later at a completed boundary.
    pub fn new_raster(
        scene: Scene,
        size: [u32; 2],
        extensions: &[std::ffi::CString],
        cache: impl Into<PathBuf>,
    ) -> Result<Self> {
        Self::new_device(scene, size, extensions, cache.into(), true)
    }
    fn new_device(
        scene: Scene,
        size: [u32; 2],
        extensions: &[std::ffi::CString],
        cache: PathBuf,
        raster: bool,
    ) -> Result<Self> {
        ensure!(!size.contains(&0), "Render extent is zero");
        ensure!(raster || !scene.meshes.is_empty(), "Scene has no geometry");
        let mut gpu = if raster {
            VulkanDevice::new_raster(extensions, cache)?
        } else {
            VulkanDevice::new(extensions, cache)?
        };
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
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
    /// Whether color and IDs belong to a completed frame and can be attached to
    /// the editor. Fresh/failed/resized targets must never be sampled or copied.
    pub fn presentation_ready(&self) -> bool {
        [OUTPUT, ID].iter().all(|&slot| {
            self.targets.states[slot].access == Access::CopySource
                && self.targets.states[slot].queue == Queue::Graphics
        })
    }
    /// Last completed entity-ID texture, in CopySource layout after rendering.
    /// The editor samples this on the same device for selection/review outlines.
    pub fn entity_channel(&self) -> Image {
        self.targets.images[ID]
    }
    /// Compact channel index (slice index + 1) to external entity identity and label.
    pub fn entities(&self) -> &[(u64, String)] {
        &self.content.entity_names
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
        if let Err(error) = self.look.resize(&mut self.gpu, size) {
            for image in replacement.images {
                self.gpu.retire(Resource::Image(image))?;
            }
            for buffer in [
                replacement.tiles,
                replacement.read_color,
                replacement.read_ids,
            ] {
                self.gpu.retire(Resource::Buffer(buffer))?;
            }
            self.gpu.collect()?;
            return Err(error);
        }
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
    /// Select the display curve applied after exposure (Lit view only).
    pub fn set_tone_curve(&mut self, curve: post::ToneCurve) {
        self.look.look.curve = curve;
    }
    /// Apply an authored look (exposure, display, sky and fog) from the next frame.
    pub fn set_look(&mut self, look: crate::look::Look) -> Result<()> {
        look.validate()?;
        self.look.look = look;
        Ok(())
    }
    /// The look applied to rendered frames.
    pub fn look(&self) -> &crate::look::Look {
        &self.look.look
    }
    /// Apply per-preset raster budgets (shadow cascades, probe GI, bloom) from the
    /// next frame. A new cascade count or resolution reallocates the cascade maps.
    pub fn set_quality(&mut self, quality: crate::look::Quality) -> Result<()> {
        quality.validate()?;
        let previous = std::mem::replace(&mut self.look.quality, quality);
        if let Err(error) = self.ensure_cascades() {
            self.look.quality = previous;
            return Err(error);
        }
        let far = quality.shadows.far;
        if far != self.look.far.as_ref().map(|f| f.settings) {
            if let Some(old) = self.look.far.take() {
                old.destroy(&mut self.gpu)?;
            }
            if let Some(far) = far {
                self.look.far = Some(crate::look::far::FarMap::new(
                    &mut self.gpu,
                    far,
                    std::mem::size_of::<frame::Frame>() as u64,
                )?);
            }
        }
        self.look.quality = quality;
        Ok(())
    }
    /// One cached map per cascade (look::cascades) at the quality's budget.
    fn ensure_cascades(&mut self) -> Result<()> {
        let shadows = self.look.quality.shadows;
        let current = self
            .look
            .cascades
            .as_ref()
            .map(|c| (c.images.len(), c.resolution));
        if current != Some((shadows.cascades as usize, shadows.resolution)) {
            if let Some(old) = self.look.cascades.take() {
                old.destroy(&mut self.gpu)?;
            }
            self.look.cascades = Some(crate::look::cascades::Cascades::new(
                &mut self.gpu,
                shadows.cascades,
                shadows.resolution,
            )?);
        }
        Ok(())
    }
    /// The per-preset raster budgets in effect.
    pub fn quality(&self) -> &crate::look::Quality {
        &self.look.quality
    }
    /// Render live with TAA jitter and camera reprojection. `exposure` is the linear
    /// multiplier of a manual look; automatic looks meter instead.
    pub fn render(&mut self, camera: &CameraSpec, exposure: f32) -> Result<()> {
        #[cfg(feature = "profiling")]
        ara_profile::scope!("render.frame");
        self.render_frame(
            camera,
            exposure,
            false,
            crate::capture::ViewMode::Lit,
            false,
        )?;
        Ok(())
    }
    /// Render live in a debug view mode (base color, normals, depth, …), as captures do,
    /// without a readback. Editors use it for their viewport View Mode menu.
    pub fn render_view(
        &mut self,
        camera: &CameraSpec,
        exposure: f32,
        mode: crate::capture::ViewMode,
    ) -> Result<()> {
        self.render_frame(camera, exposure, false, mode, false)?;
        Ok(())
    }
    /// Render an agent request, with reset history and an explicit settle frame count.
    pub fn capture(&mut self, request: &CaptureRequest) -> Result<Capture> {
        self.resize(request.size)?;
        self.reset_capture_history()?;
        ensure!(request.settle_frames <= 1024, "Settle frame limit is 1024");
        for _ in 0..request.settle_frames.max(1) {
            self.render_frame(
                &request.camera,
                request.exposure,
                request.deterministic,
                request.mode,
                true,
            )?;
        }
        self.capture_completed(request)
    }
    /// Restart temporal convergence without allocating another view or changing
    /// the authored exposure mode. Explicit captures use this on the live view.
    pub fn reset_temporal_history(&mut self) {
        self.targets.history = None;
        self.frame_number = 0;
    }
    /// Reset capture convergence, including the exposure state (it scales the
    /// pre-exposed history) and relit GI bounce history. The baked surfaces and
    /// their residency remain intact; live camera changes use the cheaper
    /// temporal-only reset instead.
    pub fn reset_capture_history(&mut self) -> Result<()> {
        self.reset_temporal_history();
        self.look.reset_exposure(&self.gpu)?;
        let Some(gi) = &mut self.look.gi else {
            return Ok(());
        };
        let rows = gi.probes.div_ceil(gi.atlas);
        let zeros = vec![0u8; (gi.atlas * 8 * rows * 8) as usize * 8];
        let source =
            self.gpu
                .create_buffer(zeros.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        self.gpu.upload(source, &zeros)?;
        let mut commands = self.gpu.begin(Queue::Graphics);
        for (image, access) in gi.irradiance.into_iter().zip(&mut gi.access) {
            let barrier = |before, after| ara_gpu::Barrier {
                resource: Resource::Image(image),
                before,
                after,
                handoff: None,
            };
            commands.barriers([barrier(*access, Access::CopyDestination)]);
            commands.push(ara_gpu::commands::Command::UploadImage {
                source,
                destination: image,
            });
            commands.barriers([barrier(Access::CopyDestination, Access::ShaderRead)]);
            *access = Access::ShaderRead;
        }
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        self.gpu.retire(Resource::Buffer(source))?;
        gi.slot = 0;
        gi.cursor = 0;
        Ok(())
    }
    /// Read the completed live frame once, including its IDs and native metadata.
    /// The caller settles particles, automatic exposure and temporal AA first.
    pub fn capture_completed(&mut self, request: &CaptureRequest) -> Result<Capture> {
        ensure!(
            request.size == self.size,
            "capture extent differs from the completed frame"
        );
        self.readback(true)?;
        let rgba = self.gpu.read(self.targets.read_color)?;
        let ids = self
            .gpu
            .read(self.targets.read_ids)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u32)
            .collect::<Vec<_>>();
        let hash = self.scene_hash();
        Capture::from_render(
            rgba,
            ids,
            request,
            crate::capture::CaptureContext {
                names: &self.content.entity_names,
                hash: &hash,
                device: &self.gpu.context().info,
                timings: &self.last_timings,
                shaders: &self.shader_metadata,
            },
        )
    }
    /// View targets; the shadow atlas holds 2x2 cascades of `shadow` texels.
    fn targets(gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<Targets> {
        let mut allocated = Vec::new();
        let result = (|| {
            let mut graph = Graph::default();
            let mut images = Vec::new();
            let mut sampled = Vec::new();
            for target in 0..TARGET_COUNT {
                let format = match target {
                    ID => vk::Format::R32_SFLOAT,
                    DEPTH => vk::Format::D32_SFLOAT,
                    OUTPUT => vk::Format::R8G8B8A8_UNORM,
                    MOTION => vk::Format::R16G16_SFLOAT,
                    // Pre-exposed (look.slangh exposureState), so half floats suffice.
                    HISTORY0 | HISTORY1 => vk::Format::R16G16B16A16_SFLOAT,
                    _ => vk::Format::R32G32B32A32_SFLOAT,
                };
                let depth = target == DEPTH;
                let usage = vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | if depth {
                        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    } else if (HISTORY0..=OUTPUT).contains(&target) {
                        vk::ImageUsageFlags::STORAGE
                    } else {
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                    };
                let (_, image) = graph.transient(
                    gpu,
                    TransientImage {
                        size,
                        format,
                        usage,
                    },
                    Queue::Graphics,
                )?;
                allocated.push(Resource::Image(image));
                images.push(image);
                sampled.push(gpu.sampled_image(image, depth)?.index);
            }
            let storage = [
                gpu.storage_image(images[HISTORY0])?.index,
                gpu.storage_image(images[HISTORY1])?.index,
                gpu.storage_image(images[OUTPUT])?.index,
            ];
            let tiles = gpu.create_buffer(
                u64::from(size[0].div_ceil(16))
                    * u64::from(size[1].div_ceil(16))
                    * (crate::scene::TILE_LIGHTS as u64 + 1)
                    * 4,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                false,
            )?;
            allocated.push(Resource::Buffer(tiles));
            let tile_index = gpu.storage_buffer(tiles)?.index;
            let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
            let read_color = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST, true)?;
            allocated.push(Resource::Buffer(read_color));
            let read_ids = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST, true)?;
            allocated.push(Resource::Buffer(read_ids));
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
        })();
        if result.is_err() {
            for resource in allocated.into_iter().rev() {
                gpu.retire(resource)?;
            }
            gpu.collect()?;
        }
        result
    }
}
