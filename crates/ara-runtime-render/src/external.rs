//! The renderer behind the presenter (in practice Renderlab). A host installs one with
//! [`Presenter::set_external_renderer`]; every present and capture draws the extracted
//! frame with it and publishes its image through the presenter's output, so the game UI,
//! overlays and the window copy work unchanged.
//!
//! The presenter owns the device the image must arrive on; the renderer owns everything
//! else (its own device, scene and history).
use crate::{
    GpuFrameCosts, PresentStats, PresentationImage, Presenter, RenderDebug, RenderFrame,
    fidelity::{EnvironmentMap, native::Content},
};
use anyhow::{Context, Result};
use ara_frame::capture::{Capture, CaptureRequest, FeatureCapture, ShaderMetadata, ViewMode};
use ara_gpu::{GpuDevice, Image, commands::PassTiming};
use ara_render_fidelity::environment::FrameEnvironment;
use ara_runtime::{extract::Draw, material::MaterialAsset};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

/// One frame for an external renderer.
pub struct ExternalView<'a> {
    /// The extracted world.
    pub frame: &'a RenderFrame,
    /// Display size in pixels; the returned image has exactly this extent.
    pub size: [u32; 2],
    /// The look the presenter resolved for this frame: authored sky, fog, exposure
    /// and display, after host overrides and the user's graphics settings.
    pub look: &'a FrameEnvironment,
    /// The content's authored environment map (an HDRI sky), when it has one.
    pub environment: Option<&'a EnvironmentMap>,
    /// The parts of each foliage rule's model or prefab, keyed by the rule's asset
    /// reference (`Content::foliage_parts`): what `frame.terrain`'s foliage instances
    /// draw. Each part's `world` is its transform inside one instance.
    pub foliage_parts: &'a BTreeMap<String, Vec<Draw>>,
    /// The terrain's splat-layer materials, keyed by the references in its header
    /// (`Content::terrain_materials`); a terrain without them has Ara's default surface.
    pub terrain_materials: &'a BTreeMap<String, Arc<MaterialAsset>>,
    /// The host turned rendering effects off or asked for a diagnostic view
    /// (`Presenter::set_debug`); an external renderer that cannot honour that says so.
    pub debug_switches: bool,
    /// Share of foliage instances to draw, 0 to 100, from the user's graphics tier: an
    /// instance draws when its identity (`identity[0] | identity[1] << 32`) modulo 100 is
    /// below it, as Ara's renderer selects them.
    pub foliage_percent: u32,
    /// Distance in metres to which terrain layers use their normal and metal/roughness
    /// maps and project from the side on steep slopes, from the user's graphics tier as
    /// Ara's renderer does (`Quality::terrain_detail`; 0: neither, anywhere).
    pub terrain_detail: f32,
}

/// A completed frame from an external renderer.
pub struct ExternalFrame {
    /// Display-encoded RGBA8 (`R8G8B8A8_UNORM` holding sRGB values) of the view's
    /// size on the presenter's device, in `CopySource`. The external renderer keeps
    /// ownership; the presenter copies it before the next `render`.
    pub image: Image,
    /// Packed object ids for this frame, RGBA8 UNORM of `image`'s size, in
    /// `CopySource`, when the handoff delivered one (little-endian ids, 0 nothing,
    /// 0xFFFF_FFFF a hit without an id). `None` when the renderer has no id image:
    /// the host rasterises its own.
    pub id: Option<Image>,
    /// The same packed ids as RGBA8 texels, rows top first with no padding, when the renderer
    /// delivered them by CPU copy (what a click reads, bit for bit). `None` on the shared
    /// handoff, where `id` is the only copy.
    pub id_texels: Option<Vec<u8>>,
    /// GPU milliseconds the external renderer measured for a recent frame, when it
    /// has timestamps (it may lag the presented frame).
    pub gpu_ms: Option<f64>,
    /// Wall milliseconds spent moving the image onto the presenter's device.
    pub handoff_ms: f64,
    /// GPU milliseconds of the renderer's own named passes for the frame `gpu_ms`
    /// measured, where it reports them; entries with an empty name are unused.
    /// Captures list them as `External.Pass.<name>`, so the span minus their sum is
    /// what the renderer spent outside them.
    pub passes: [(&'static str, f64); EXTERNAL_PASSES],
}

/// Named passes an external renderer can report per frame (`ExternalFrame::passes`).
pub const EXTERNAL_PASSES: usize = 6;

/// A renderer that draws extracted frames.
pub trait ExternalRenderer: Send {
    /// Name for logs and capture provenance.
    fn name(&self) -> &str;
    /// Draws `view` and returns its completed image on `gpu`.
    fn render(&mut self, gpu: &mut GpuDevice, view: &ExternalView<'_>) -> Result<ExternalFrame>;
    /// The next frame starts without temporal history (a capture or camera cut).
    fn cut(&mut self);
    /// Frames a still capture renders before it reads back, so temporal
    /// accumulation and lighting caches converge.
    fn settle_frames(&self) -> u32;
    /// Releases what it created on `gpu`, before that device is destroyed.
    fn release(&mut self, gpu: &mut GpuDevice) -> Result<()>;
    /// Starts drawing `view` and returns without waiting for the GPU, for a host that must not
    /// block on a frame: `Ok(true)` when it did (take the oldest result with [`Self::finish`]; up
    /// to [`Self::max_frames_in_flight`] frames are in flight, see [`Self::frames_in_flight`]),
    /// `Ok(false)` when the renderer cannot split a frame and the host calls [`Self::render`]
    /// instead.
    fn begin(&mut self, _gpu: &mut GpuDevice, _view: &ExternalView<'_>) -> Result<bool> {
        Ok(false)
    }
    /// Whether a frame was begun and not yet taken.
    fn in_flight(&self) -> bool {
        false
    }
    /// How many frames were begun and not yet taken.
    fn frames_in_flight(&self) -> usize {
        usize::from(self.in_flight())
    }
    /// How many frames [`Self::begin`] accepts before one is taken: more than one lets the host
    /// record the next frame while the GPU draws the last.
    fn max_frames_in_flight(&self) -> usize {
        1
    }
    /// Takes the oldest frame [`Self::begin`] started once the GPU has finished it; `None` while
    /// it is still drawing. Never waits for the GPU.
    fn finish(&mut self, _gpu: &mut GpuDevice) -> Result<Option<ExternalFrame>> {
        Ok(None)
    }
    /// The id of the frontmost surface (glass included) under `pixel` of the last frame, from
    /// the renderer's own id pass: what a click picks. `0` is nothing and `0xFFFF_FFFF` a surface
    /// without an id. `None`: it has no ids, or could not answer in time, and the host reads
    /// the delivered id image instead.
    fn pick_id(&mut self, _pixel: [u32; 2]) -> Result<Option<u32>> {
        Ok(None)
    }
}

impl Presenter {
    /// Draws every following present and capture with `external` (see the module
    /// docs); `None` removes the renderer, and presents fail until another is installed.
    pub fn set_external_renderer(
        &mut self,
        external: Option<Box<dyn ExternalRenderer>>,
    ) -> Result<()> {
        if let Some(mut old) = self.external.take()
            && let Some(device) = self.device.as_mut()
        {
            old.release(device.gpu_mut())?;
        }
        self.external = external;
        Ok(())
    }

    /// Whether presents show the external renderer's frame one behind (default off): the CPU
    /// records frame N while the GPU draws frame N-1, so a present costs the longer of the two
    /// instead of their sum, at the price of a frame of latency. It only takes effect with a
    /// renderer that allows more than one frame in flight (`max_frames_in_flight`), and captures
    /// always draw and read the frame they ask for.
    pub fn set_external_pipelined(&mut self, on: bool) {
        self.external_pipelined = on;
    }

    /// Name of the installed external renderer, if any.
    pub fn external_renderer(&self) -> Option<&str> {
        self.external.as_deref().map(ExternalRenderer::name)
    }

    /// Renders `frame` with the external renderer and publishes its image.
    /// `resolve` runs only when the draw count changes, for the environment map, the
    /// foliage parts and the terrain layer materials.
    pub(crate) fn present_external(
        &mut self,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        self.stats = PresentStats::default();
        let started = Instant::now();
        self.ensure_device()?;
        anyhow::ensure!(
            self.external.is_some(),
            "no renderer is installed on this presenter"
        );
        // The draw count stands in for a structural change: ordinary motion never
        // resolves content.
        if self.external_draws != Some(frame.draws.len()) {
            let content = resolve()?;
            self.stats.prepare = started.elapsed();
            self.environment = content.environment;
            self.external_foliage = content.foliage_parts;
            self.external_terrain_materials = content.terrain_materials;
            self.external_draws = Some(frame.draws.len());
        }
        let look = self.frame_look(frame);
        let size = self.size;
        // The exposure lock reaches the external renderer through the look.
        let debug_switches = self.debug
            != RenderDebug {
                exposure: self.debug.exposure,
                ..Default::default()
            };
        let foliage_percent = self.user_content_budget().1;
        let terrain_detail = self.user_terrain_detail();
        let gpu = self
            .device
            .as_mut()
            .context("presentation device")?
            .gpu_mut();
        let render_started = Instant::now();
        let external = self.external.as_mut().context("external renderer")?;
        let view = ExternalView {
            frame,
            size,
            look: &look,
            environment: self.environment.as_ref(),
            foliage_parts: &self.external_foliage,
            terrain_materials: &self.external_terrain_materials,
            debug_switches,
            foliage_percent,
            terrain_detail,
        };
        let rendered = if self.external_pipelined && external.max_frames_in_flight() > 1 {
            render_one_behind(external.as_mut(), gpu, &view)?
        } else {
            external.render(gpu, &view)?
        };
        let publication_ms =
            PresentationImage::publish(&mut self.published, gpu, rendered.image, size)?;
        let costs = GpuFrameCosts {
            raster_ms: rendered.gpu_ms.unwrap_or(0.),
            publication_ms,
            ..Default::default()
        };
        self.stats.gpu_ms = costs.accounted_ms();
        self.stats.gpu = Some(costs);
        self.stats.external_handoff_ms = Some(rendered.handoff_ms);
        self.external_passes = rendered.passes;
        self.timings = timings(costs.raster_ms, &rendered.passes);
        if let Some(frame) = self.profile_next.take() {
            if self.profile_completed.len() == 8 {
                self.profile_completed.pop_front();
            }
            self.profile_completed
                .push_back((frame, self.timings.clone()));
        }
        self.stats.render = render_started.elapsed();
        Ok(())
    }

    /// A still of `frame` from the external renderer: history is cut, the frame
    /// settles, and the published image is read back. Entity marks need Ara's ID
    /// buffer, so the legend is empty.
    pub(crate) fn capture_external(
        &mut self,
        frame: &RenderFrame,
        request: &CaptureRequest,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<Capture> {
        let external = self.external.as_mut().context("external renderer")?;
        // A debug view would otherwise show the lit image under its name.
        anyhow::ensure!(
            matches!(request.mode, ViewMode::Lit),
            "ARA_VIEW_MODE_UNAVAILABLE: {:?} view unavailable: the {} renderer draws the lit image only",
            request.mode,
            external.name()
        );
        anyhow::ensure!(
            self.debug.reference.is_none(),
            "ARA_VIEW_MODE_UNAVAILABLE: the path-traced reference is unavailable: the {} renderer draws its own real-time image only",
            external.name()
        );
        // A still shows the frame it asks for: frames in flight from pipelined presents go first.
        while external.in_flight() {
            let _ = external.finish(
                self.device
                    .as_mut()
                    .context("presentation device")?
                    .gpu_mut(),
            )?;
            std::thread::yield_now();
        }
        // The settle frames present one behind when pipelining is on (so a capture times the
        // same loop the player runs); the last one is drawn and waited for, so the still is the
        // frame asked for.
        let pipelined = self.external_pipelined;
        let external = self.external.as_mut().context("external renderer")?;
        external.cut();
        let frames = external.settle_frames().max(request.settle_frames);
        let name = external.name().to_owned();
        let mut resolve = Some(resolve);
        // Wall time of the settled frames (the second half: loading is behind them).
        let mut walls = Vec::new();
        for index in 0..=frames {
            let started = Instant::now();
            if index == frames && pipelined {
                // Frames in flight go first, then the still's own frame is drawn blocking.
                while self.external.as_ref().is_some_and(|e| e.in_flight()) {
                    let gpu = self
                        .device
                        .as_mut()
                        .context("presentation device")?
                        .gpu_mut();
                    if let Some(external) = self.external.as_mut() {
                        let _ = external.finish(gpu)?;
                    }
                    std::thread::yield_now();
                }
                self.external_pipelined = false;
            }
            self.present_external(frame, || match resolve.take() {
                Some(resolve) => resolve(),
                None => Ok(Content::default()),
            })?;
            // The still's own blocking frame is not a pipelined frame time.
            if index > frames / 2 && !(pipelined && index == frames) {
                walls.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        self.external_pipelined = pipelined;
        walls.sort_by(f64::total_cmp);
        // The capture's timings name what the external path measured: its own GPU
        // span for a settled frame, then wall medians (not device timestamps).
        let timing = |pass: &str, milliseconds: f64| PassTiming {
            pass: pass.into(),
            queue: ara_gpu::Queue::Graphics,
            milliseconds,
            start_ms: 0.,
            end_ms: milliseconds,
        };
        let costs = self.stats.gpu.unwrap_or_default();
        let mut timings = vec![
            timing("External.Frame.Gpu", costs.raster_ms),
            timing("External.Publish.Gpu", costs.publication_ms),
            timing(
                "External.Handoff.Wall",
                self.stats.external_handoff_ms.unwrap_or(0.),
            ),
            timing(
                "External.Frame.WallMedian",
                walls.get(walls.len() / 2).copied().unwrap_or(0.),
            ),
        ];
        for (name, milliseconds) in self.external_passes {
            if !name.is_empty() {
                timings.push(timing(&format!("External.Pass.{name}"), milliseconds));
            }
        }
        let image = self
            .published
            .as_ref()
            .context("external capture published no image")?
            .output();
        let rgba = crate::readback::read_rgba8(
            self.device
                .as_mut()
                .context("presentation device")?
                .gpu_mut(),
            image,
            request.size,
        )?;
        let pixels = rgba.len() / 4;
        let info = self
            .device
            .as_ref()
            .context("presentation device")?
            .gpu()
            .info();
        Capture::from_feature(
            FeatureCapture {
                rgba,
                ids: vec![0; pixels],
                names: &[],
                hash: &name,
                device: info,
                timings: &timings,
                // The external renderer compiles its own shaders; no Ara cook applies.
                shaders: &ShaderMetadata {
                    content_key: name.clone(),
                    compiler: String::new(),
                    binding_layout_hash: String::new(),
                },
            },
            request,
        )
    }
}

impl Presenter {
    /// The frame the last `present` displayed, read back for live measurements
    /// (stability, video): the published image, as it is.
    pub fn live_image(&mut self) -> Result<image::RgbaImage> {
        let published = self.published.as_ref().context("no presented frame")?;
        let size = published.size();
        let output = published.output();
        let rgba = crate::readback::read_rgba8(
            self.device
                .as_mut()
                .context("presentation device")?
                .gpu_mut(),
            output,
            size,
        )?;
        image::RgbaImage::from_raw(size[0], size[1], rgba).context("presented image dimensions")
    }
}

/// The frame's GPU timings as pass records: the renderer's whole frame, then its named
/// passes (entries with an empty name are unused).
/// Begins `view` and takes the oldest frame, blocking for it: the frame begun on the previous
/// call stays in flight (and so does this one), so the CPU records the next while the GPU draws
/// the last. The first call begins a second frame of the same view to fill the pipeline.
fn render_one_behind(
    external: &mut dyn ExternalRenderer,
    gpu: &mut GpuDevice,
    view: &ExternalView<'_>,
) -> Result<ExternalFrame> {
    while external.frames_in_flight() >= external.max_frames_in_flight() {
        if let Some(frame) = external.finish(gpu)? {
            drop(frame);
        }
        std::thread::yield_now();
    }
    if !external.begin(gpu, view)? {
        return external.render(gpu, view);
    }
    if external.frames_in_flight() < 2 {
        external.begin(gpu, view)?;
    }
    loop {
        if let Some(frame) = external.finish(gpu)? {
            return Ok(frame);
        }
        std::thread::yield_now();
    }
}

fn timings(frame_ms: f64, passes: &[(&'static str, f64); EXTERNAL_PASSES]) -> Vec<PassTiming> {
    let timing = |pass: &str, milliseconds: f64| PassTiming {
        pass: pass.into(),
        queue: ara_gpu::Queue::Graphics,
        milliseconds,
        start_ms: 0.,
        end_ms: milliseconds,
    };
    std::iter::once(timing("External.Frame", frame_ms))
        .chain(
            passes
                .iter()
                .filter(|(name, _)| !name.is_empty())
                .map(|(name, ms)| timing(&format!("External.Pass.{name}"), *ms)),
        )
        .collect()
}
