//! Renderer orchestration. Extraction snapshots are uploaded once; frames record handles.
mod edit;
mod frame;
mod gi;
mod hash;
mod live;
mod native;
mod passes;
pub mod post;
mod readback;
mod readiness;
mod submission;
mod transfer;
mod upload;
mod views;
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
pub use views::{ViewId, ViewOutput, ViewRequest};

const ALBEDO: usize = 0;
const NORMAL: usize = 1;
const MATERIAL: usize = 2;
const EMISSIVE: usize = 3;
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
    /// Display size: TAA history, output and captures.
    size: [u32; 2],
    /// Render size of the scene passes (G-buffer, depth, HDR, IDs, motion); smaller
    /// than `size` with temporal upscaling (`resize_upscaled`).
    render: [u32; 2],
    targets: Targets,
    pipelines: Pipelines,
    content: upload::Content,
    frame: Buffer,
    frame_index: u32,
    /// The frame record of display-size passes (temporal resolve, post, debug
    /// views): `frame` with the display extent.
    display_frame: Buffer,
    display_frame_index: u32,
    compute_frame: Buffer,
    compute_frame_index: u32,
    /// Unjittered view-projection of the last rendered frame (`Frame::previous_vp`).
    previous_vp: Mat4,
    frame_number: u32,
    artifacts: PathBuf,
    shader_metadata: crate::capture::ShaderMetadata,
    last_timings: Vec<PassTiming>,
    /// Sky, fog, exposure, bloom and display grading after the G-buffer.
    look: crate::look::gpu::LookGpu,
    native: Option<native::Native>,
    views: views::Views,
    secondary_active: bool,
    live: Option<live::Live>,
    entity_epoch: u64,
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
        let targets = Self::targets(&mut gpu, size, size)?;
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
    /// Display size (history, output, captures and picking); resize replaces
    /// history and retires old graph storage.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// Render size of the scene passes and of `feature_color`, `feature_depth` and
    /// `entity_channel`: the display size unless temporally upscaled.
    pub fn render_size(&self) -> [u32; 2] {
        self.render
    }
    /// Current SDR output, in CopySource layout after rendering.
    pub fn output(&self) -> Image {
        if self.secondary_active {
            return self.targets.images[OUTPUT];
        }
        self.live
            .as_ref()
            .and_then(live::Live::color)
            .unwrap_or(self.targets.images[OUTPUT])
    }
    /// Extent of `output()`, which may retain the previous size during an
    /// asynchronous resize until its replacement frame has completed.
    pub fn output_size(&self) -> [u32; 2] {
        if self.secondary_active {
            return self.size;
        }
        self.live
            .as_ref()
            .and_then(live::Live::size)
            .unwrap_or(self.size)
    }
    /// Whether color and IDs belong to a completed frame and can be attached to
    /// the editor. Fresh/failed/resized targets must never be sampled or copied.
    pub fn presentation_ready(&self) -> bool {
        if !self.secondary_active
            && let Some(live) = &self.live
        {
            return live.color().is_some();
        }
        [OUTPUT, ID].iter().all(|&slot| {
            self.targets.states[slot].access == Access::CopySource
                && self.targets.states[slot].queue == Queue::Graphics
        })
    }
    /// Last completed entity-ID texture (render size), in CopySource layout after
    /// rendering. The editor samples this on the same device for selection/review outlines.
    pub fn entity_channel(&self) -> Image {
        if self.secondary_active {
            return self.targets.images[ID];
        }
        self.live
            .as_ref()
            .and_then(live::Live::ids)
            .unwrap_or(self.targets.images[ID])
    }
    /// Compact channel index (slice index + 1) to external entity identity and label.
    pub fn entities(&self) -> &[(u64, String)] {
        if self.secondary_active {
            return &self.content.entity_names;
        }
        self.live
            .as_ref()
            .and_then(live::Live::labels)
            .unwrap_or(&self.content.entity_names)
    }
    /// Last measured pass timings, including actual dedicated-queue execution.
    pub fn timings(&self) -> &[PassTiming] {
        &self.last_timings
    }

    /// Install the view's completed pass timings after a presenter-owned deferred
    /// scope. Feature passes are accounted by their owner, not added to this view.
    pub fn set_completed_timings(&mut self, timings: Vec<PassTiming>) {
        self.last_timings = timings;
    }
    /// Apply a changed display extent at native resolution. An unchanged window
    /// notification preserves its active render size and temporal history; use
    /// `resize_upscaled(size, size)` to explicitly switch an upscaled view to native.
    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        if size == self.size {
            if let Some(live) = &mut self.live
                && live.resize.is_some_and(|(_, display)| display != size)
            {
                // A window can return to its current extent before the pending
                // resize is adopted. Keep the still-valid render allocation.
                live.resize = Some((self.render, size));
            }
            return Ok(());
        }
        self.resize_upscaled(size, size)
    }
    /// Temporal upscaling (TAAU): the scene passes (G-buffer, lighting, the
    /// half-resolution effects, IDs) run at `render`, and the temporal resolve
    /// reconstructs the `display`-sized history from jittered render samples;
    /// exposure, bloom and grading run at `display`. Equal sizes render natively.
    /// Resets temporal history.
    pub fn resize_upscaled(&mut self, render: [u32; 2], display: [u32; 2]) -> Result<()> {
        ensure!(
            !display.contains(&0) && !render.contains(&0),
            "Cannot render a minimized window"
        );
        ensure!(
            render[0] <= display[0] && render[1] <= display[1],
            "Render size {render:?} exceeds the display {display:?}"
        );
        if let Some(live) = &mut self.live
            && live.pending()
        {
            live.resize = Some((render, display));
            return Ok(());
        }
        if render == self.render && display == self.size {
            return Ok(());
        }
        ara_gpu::cpu_scope!("Raster.resize_targets");
        let replacement = Self::targets(&mut self.gpu, render, display)?;
        if let Err(error) = self.look.resize(&mut self.gpu, render, display) {
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
        self.size = display;
        self.render = render;
        self.frame_number = 0;
        Ok(())
    }
    /// Applied linear exposure of the last completed frame (automatic exposure's
    /// adapted multiplier), or `None` before any metered frame. Read it between
    /// completed frames, e.g. to seed another view's start (`seed_exposure`).
    pub fn exposure(&self) -> Result<Option<f32>> {
        let state = self.gpu.read(self.look.state)?;
        let value = f32::from_le_bytes(state[..4].try_into()?);
        Ok((value.is_finite() && value > 0.).then_some(value))
    }
    /// Start the next frame's exposure from `multiplier` (a linear exposure, as
    /// `exposure` returns): automatic exposure adapts from it at the look's rate
    /// instead of snapping to its first meter, and history pre-exposes with it.
    /// Hosts carry a converged exposure across views (Game preview into Play).
    /// Capture resets still start from zero and snap. Inside a deferred Scene
    /// submission the write is GPU-ordered (`upload_queued`); outside one, call it
    /// with no frame in flight.
    pub fn seed_exposure(&mut self, multiplier: f32) -> Result<()> {
        ensure!(
            multiplier.is_finite() && multiplier > 0.,
            "Exposure seed must be a finite positive multiplier"
        );
        self.look.seed_exposure(&mut self.gpu, multiplier)
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
        let local = quality.local_shadows;
        if local != self.look.local.as_ref().map(|l| l.settings) {
            if let Some(old) = self.look.local.take() {
                old.destroy(&mut self.gpu)?;
            }
            if let Some(local) = local {
                self.look.local = Some(crate::look::local::LocalMap::new(&mut self.gpu, local)?);
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
                std::mem::size_of::<frame::Frame>() as u64,
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
    /// pre-exposed history) and relit GI bounce history: for the next
    /// `look::gi::CONVERGE_FRAMES` every probe relights and exposure re-meters,
    /// with the volume shown at once. Also the local-shadow selection. The baked
    /// surfaces and
    /// their residency remain intact; live camera changes use the cheaper
    /// temporal-only reset instead.
    pub fn reset_capture_history(&mut self) -> Result<()> {
        self.reset_temporal_history();
        self.look.reset_exposure(&mut self.gpu)?;
        self.look.settle = crate::look::gi::CONVERGE_FRAMES;
        // Local shadows start over: which lights a capture shadows must not
        // depend on the lights live frames held.
        if let Some(local) = &mut self.look.local {
            local.cache = crate::look::local::LocalCache::new(local.settings);
        }
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
        gi.converge = crate::look::gi::CONVERGE_FRAMES;
        gi.show();
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
        let low = self
            .gpu
            .read(self.targets.read_ids)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u32)
            .collect::<Vec<_>>();
        // Upscaled IDs map each display pixel to its render pixel (nearest).
        let [width, height] = self.size;
        let ids = if self.render == self.size {
            low
        } else {
            (0..height)
                .flat_map(|y| (0..width).map(move |x| [x, y]))
                .map(|pixel| low[self.render_pixel(pixel)])
                .collect()
        };
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
    /// Row-major index of the render pixel under display pixel `pixel`.
    fn render_pixel(&self, pixel: [u32; 2]) -> usize {
        let [x, y] = std::array::from_fn(|i| {
            (u64::from(pixel[i]) * u64::from(self.render[i]) / u64::from(self.size[i])) as usize
        });
        y * self.render[0] as usize + x
    }
    /// View targets: the compact G-buffer (decoded by raster.slangh's gbuffer*
    /// helpers), depth, ID, HDR and motion at the `render` size; TAA histories and
    /// output at `display`.
    fn targets(gpu: &mut VulkanDevice, render: [u32; 2], display: [u32; 2]) -> Result<Targets> {
        let mut allocated = Vec::new();
        let result = (|| {
            let mut graph = Graph::default();
            let mut images = Vec::new();
            let mut sampled = Vec::new();
            for target in 0..TARGET_COUNT {
                let format = match target {
                    // The compact G-buffer (raster.slangh gbuffer* helpers).
                    ALBEDO => vk::Format::R8G8B8A8_SRGB,
                    NORMAL => vk::Format::R16G16_UNORM,
                    MATERIAL => vk::Format::R8G8B8A8_UNORM,
                    EMISSIVE => vk::Format::B10G11R11_UFLOAT_PACK32,
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
                let size = if (HISTORY0..=OUTPUT).contains(&target) {
                    display
                } else {
                    render
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
                u64::from(render[0].div_ceil(16))
                    * u64::from(render[1].div_ceil(16))
                    * (crate::scene::TILE_LIGHTS as u64 + 1)
                    * 4,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                false,
            )?;
            allocated.push(Resource::Buffer(tiles));
            let tile_index = gpu.storage_buffer(tiles)?.index;
            let bytes = |[w, h]: [u32; 2]| u64::from(w) * u64::from(h) * 4;
            let read_color =
                gpu.create_buffer(bytes(display), vk::BufferUsageFlags::TRANSFER_DST, true)?;
            allocated.push(Resource::Buffer(read_color));
            let read_ids =
                gpu.create_buffer(bytes(render), vk::BufferUsageFlags::TRANSFER_DST, true)?;
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
