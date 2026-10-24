//! Embedded players (D19): no window. A headless presenter renders at the size the
//! owner asked for, and each frame is copied into a ring of exportable images that
//! the owner samples on its own device (`ara_gpu::shared` has the protocol). The
//! owner learns of each frame through `play.frame` once its GPU work completed and
//! hands slots back with `play.release`. Raw input arrives as `play.device` and goes
//! through the same [`Mapper`] as windowed input.
use crate::{device, input::DeviceEvent, input::Mapper};
use anyhow::{Context, Result, ensure};
use ara_gpu::{
    shared::{Frame, FrameReady, SurfaceDesc, now_us},
    vulkan::external::ExportRing,
};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::Presenter;
use serde::Deserialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Embedding methods and notifications, mirrored by `ara_play_host::embed::method`.
pub mod method {
    /// Owner → player request before `play.start`: `{width, height, maxFps?}` →
    /// the surface (`SurfaceDesc`, handle values in the player's process).
    pub const EMBED: &str = "play.embed";
    /// Owner → player `{width, height}`: as a request, answers the new surface; as a
    /// notification, the player answers with [`SURFACE`].
    pub const RESIZE: &str = "play.resize";
    /// Player → owner notification: a new surface generation (`SurfaceDesc`).
    pub const SURFACE: &str = "play.surface";
    /// Player → owner notification: a frame completed (`FrameReady`).
    pub const FRAME: &str = "play.frame";
    /// Owner → player notification: the owner returned a frame's slot (`Frame`).
    pub const RELEASE: &str = "play.release";
    /// Owner → player `{events}` raw device input (request → `{accepted}`, or a
    /// notification).
    pub const DEVICE: &str = "play.device";
}

/// `play.embed` params.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbedParams {
    /// Surface width in pixels.
    pub width: u32,
    /// Surface height in pixels.
    pub height: u32,
    /// Most frames rendered per second (default 60).
    #[serde(default)]
    pub max_fps: Option<u32>,
}

/// `play.resize` params.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResizeParams {
    /// New width in pixels.
    pub width: u32,
    /// New height in pixels.
    pub height: u32,
}

/// `play.device` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceParams {
    /// Raw events, applied in order.
    pub events: Vec<DeviceEvent>,
}

/// Largest surface edge accepted (the renderer's targets scale with it).
const MAX_EDGE: u32 = 8192;

fn check(size: [u32; 2]) -> Result<()> {
    ensure!(
        size.iter().all(|v| (1..=MAX_EDGE).contains(v)),
        "surface size {size:?} must be 1..={MAX_EDGE} pixels per edge"
    );
    Ok(())
}

/// An embedded player's shared ring, pacing, input and frame statistics.
pub struct Embedded {
    ring: ExportRing,
    interval: Duration,
    last: Option<Instant>,
    /// Render the next frame even if nothing paced it (new surface).
    dirty: bool,
    /// A resize requested by notification, applied once per loop (latest wins).
    pending: Option<[u32; 2]>,
    /// The owner's raw input, mapped like windowed input.
    pub input: Mapper,
    frames: u32,
    frame_times: Vec<f32>,
    since: Instant,
}

impl Embedded {
    /// Creates the headless presenter (its device and pipelines warmed with a
    /// one-box frame) and the first ring generation.
    pub fn open(
        params: &EmbedParams,
        cache: &Path,
        quality: ara_runtime_render::QualityChoice,
    ) -> Result<(Self, Presenter)> {
        let size = [params.width, params.height];
        check(size)?;
        let mut presenter = Presenter::new(size, Vec::new(), cache);
        presenter.set_quality(quality);
        presenter.prepare(&device::warm_frame())?;
        let renderer = presenter
            .renderer()
            .context("renderer missing after warm-up")?;
        // SAFETY: The presenter (and its device) outlives the ring: `Player` declares
        // `embed` before `presenter`, and this thread owns the device's queues.
        let ring = unsafe { ExportRing::new(renderer.gpu().context(), size)? };
        let fps = params.max_fps.unwrap_or(60).clamp(1, 1000);
        Ok((
            Self {
                ring,
                interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
                last: None,
                dirty: true,
                pending: None,
                input: Mapper::default(),
                frames: 0,
                frame_times: Vec::new(),
                since: Instant::now(),
            },
            presenter,
        ))
    }

    /// The current surface generation (handles valid in this process).
    pub fn desc(&self) -> SurfaceDesc {
        self.ring.desc()
    }

    /// The owner returned a frame's slot; stale frames are ignored.
    pub fn release(&mut self, frame: Frame) {
        self.ring.release(frame);
    }

    /// Resizes now: presenter targets and a new ring generation with every slot free
    /// (even at the same size: owners use that to reclaim slots a lost handle held).
    pub fn resize(&mut self, presenter: &mut Presenter, size: [u32; 2]) -> Result<SurfaceDesc> {
        check(size)?;
        self.pending = None;
        presenter.resize(size)?;
        let renderer = presenter.renderer().context("renderer missing")?;
        // SAFETY: Same device as the ring's; this thread owns its queues.
        unsafe { self.ring.resize(renderer.gpu().context(), size)? };
        self.dirty = true;
        Ok(self.ring.desc())
    }

    /// Queues a resize (latest wins) for [`Embedded::apply_pending`].
    pub fn request_resize(&mut self, size: [u32; 2]) -> Result<()> {
        check(size)?;
        self.pending = Some(size);
        Ok(())
    }

    /// Applies a queued resize; returns the new surface to announce.
    pub fn apply_pending(&mut self, presenter: &mut Presenter) -> Result<Option<SurfaceDesc>> {
        match self.pending.take() {
            Some(size) => self.resize(presenter, size).map(Some),
            None => Ok(None),
        }
    }

    /// Whether a frame is due at `now`: a slot is free and the surface is new or the
    /// frame cap allows the next frame. Due frames extract, update audio and
    /// [`Embedded::render`]. Never blocks on the owner: while it holds every other
    /// slot no frame is due (the simulation keeps ticking), and the first release
    /// makes the next one due at once.
    pub fn due(&self, now: Instant) -> bool {
        self.ring.free() > 0 && (self.dirty || self.last.is_none_or(|l| now - l >= self.interval))
    }

    /// Renders a due `frame` into a free slot; the notice to send once it returns.
    pub fn render(
        &mut self,
        presenter: &mut Presenter,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<ara_runtime_render::fidelity::native::Content>,
    ) -> Result<Option<FrameReady>> {
        let started = Instant::now();
        self.last = Some(started);
        if frame.draws.is_empty() || self.ring.free() == 0 {
            return Ok(None);
        }
        presenter.present_with(frame, resolve)?;
        let rendered_at_us = now_us();
        let copy = Instant::now();
        let renderer = presenter.renderer().context("renderer missing")?;
        let source = renderer.gpu().native_image(
            presenter
                .output()
                .context("completed presentation missing")?,
        )?;
        // SAFETY: The presenter completed this RGBA8 CopySource image at output_size
        // on the ring's device; this thread owns its queues.
        let Some(published) = (unsafe {
            self.ring
                .publish_scaled(source, presenter.output_size(), frame.tick)?
        }) else {
            return Ok(None);
        };
        let publish_us = copy.elapsed().as_micros() as u64;
        let cost = started.elapsed();
        self.dirty = false;
        self.frames += 1;
        self.frame_times.push(cost.as_secs_f32() * 1000.0);
        Ok(Some(FrameReady {
            frame: published,
            rendered_at_us,
            cost_us: cost.as_micros() as u64,
            publish_us,
        }))
    }

    /// Frames per second and p95 frame cost since the previous call.
    pub fn take_stats(&mut self) -> (f64, f64) {
        let stats = device::frame_stats(self.frames, self.since, &mut self.frame_times);
        self.frames = 0;
        self.since = Instant::now();
        stats
    }
}
