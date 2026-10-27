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
    pub(crate) game_ui: Option<crate::game_ui::Surface>,
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
        options: ara_runtime_render::RenderOptions,
        prepared: Option<Presenter>,
    ) -> Result<(Self, Presenter)> {
        let size = [params.width, params.height];
        check(size)?;
        let mut presenter = match prepared {
            Some(mut presenter) => {
                presenter.resize(size)?;
                presenter
            }
            None => Presenter::new(size, Vec::new(), cache),
        };
        presenter.set_quality(quality);
        presenter.set_render_options(options);
        presenter.set_probe_gi(false);
        presenter.prepare(&device::warm_frame())?;
        presenter.set_probe_gi(true);
        let renderer = presenter
            .renderer()
            .context("renderer missing after warm-up")?;
        // SAFETY: The presenter (and its device) outlives the ring: `Player` declares
        // `embed` before `presenter`, and this thread owns the device's queues.
        let ring = unsafe { ExportRing::new(renderer.gpu().context(), size)? };
        let fps = params.max_fps.unwrap_or(60).clamp(1, 1000);
        Ok((
            Self {
                game_ui: None,
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
        let mut size = presenter.output_size();
        let mut source = renderer.gpu().native_image(
            presenter
                .output()
                .context("completed presentation missing")?,
        )?;
        if let Some(ui) = &mut self.game_ui {
            (source, size) = ui.render(presenter)?;
        }
        // SAFETY: The presenter completed this RGBA8 CopySource image at output_size
        // on the ring's device; this thread owns its queues.
        let Some(published) = (unsafe { self.ring.publish_scaled(source, size, frame.tick)? })
        else {
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

/// Warm device and pipelines without a window or shared ring. The idle view is
/// bounded to 64x64 Low with one procedural box and no probe acquisition.
pub(crate) fn prewarm(
    cache: &Path,
    options: ara_runtime_render::RenderOptions,
) -> Result<Presenter> {
    ara_gpu::cpu_scope!("Player.prewarm_graphics");
    let mut presenter = Presenter::new([64, 64], Vec::new(), cache);
    presenter.set_quality(ara_runtime_render::QualityChoice::Preset(
        ara_runtime_render::QualityPreset::Low,
    ));
    presenter.set_render_options(options);
    presenter.set_probe_gi(false);
    presenter.prepare(&device::warm_frame())?;
    Ok(presenter)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ara_runtime_render::{QualityChoice, QualityPreset, RenderOptions};

    #[test]
    fn embedded_gpu_defaults_to_raster_and_honours_the_rt_opt_in() -> Result<()> {
        // Field order also protects assertion/error unwinding: the ring borrows
        // native device objects and must be dropped before the owning presenter.
        struct Session {
            _ring: Embedded,
            presenter: Presenter,
        }
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-player/embedded-render-options")
            .join(std::process::id().to_string());
        let params = EmbedParams {
            width: 128,
            height: 72,
            max_fps: Some(144),
        };
        for (name, options) in [
            ("raster", RenderOptions::default()),
            ("rt-opt-in", RenderOptions { ray_tracing: true }),
        ] {
            let prepared = prewarm(&root.join(name), options)?;
            assert_eq!(prepared.size(), [64, 64]);
            assert!(!prepared.probe_gi_pending());
            let device = prepared
                .renderer()
                .context("prewarmed renderer")?
                .gpu()
                .context()
                .device
                .handle();
            let (embedded, presenter) = Embedded::open(
                &params,
                &root.join(name),
                QualityChoice::Preset(QualityPreset::High),
                options,
                Some(prepared),
            )?;
            assert_eq!(
                presenter
                    .renderer()
                    .context("embedded renderer")?
                    .gpu()
                    .context()
                    .device
                    .handle(),
                device,
                "embedding reuses the presenter's logical device"
            );
            let mut session = Session {
                _ring: embedded,
                presenter,
            };
            let presenter = &mut session.presenter;
            let renderer = presenter.renderer().context("warmed embedded renderer")?;
            let rt = renderer.gpu().context().info.ray_tracing;
            if !options.ray_tracing {
                assert!(!rt, "default embedding must open a raster-only device");
                assert!(
                    renderer.is_native(),
                    "default embedding must use native raster geometry"
                );
            } else if rt {
                assert!(
                    !renderer.is_native(),
                    "supported explicit RT uses the fidelity view"
                );
            } else {
                assert!(renderer.is_native(), "unsupported RT keeps the raster path");
            }
            assert_eq!(presenter.quality(), Some(QualityPreset::High));
            renderer.gpu().assert_clean()?;
            for preset in [
                QualityPreset::Low,
                QualityPreset::Medium,
                QualityPreset::High,
            ] {
                presenter.set_quality(QualityChoice::Preset(preset));
                presenter.prepare(&device::warm_frame())?;
                let renderer = presenter.renderer().context("embedded preset renderer")?;
                assert_eq!(
                    renderer.is_native(),
                    !(options.ray_tracing && rt && preset != QualityPreset::Low)
                );
                renderer.gpu().assert_clean()?;
            }
        }
        Ok(())
    }
}
