//! Windowed players: an SDL3 window, keyboard/mouse → action injections through the
//! shared [`Mapper`] (the replayable path agents use), and per-frame presentation
//! through `ara-platform`.
use crate::input::{DeviceEvent, Mapper};
use anyhow::{Context, Result};
use ara_input::{Injection, PlayerId};
use ara_platform::{Event, Present, Window};
use ara_runtime::{
    components::{Environment, MeshShape},
    extract::{Draw, Geometry, RenderFrame},
};
use ara_runtime_render::{Presenter, two_d::Presenter2d};
use std::{
    path::Path,
    time::{Duration, Instant},
};
mod loading;
pub(crate) use loading::Loading;

/// SDL scancode of Escape: closes a windowed player.
const ESCAPE: i32 = 41;

/// The player's window and device-input state.
pub struct Windowed {
    pub(crate) game_ui: Option<crate::game_ui::Surface>,
    window: Window,
    present: Option<Present>,
    present2d: Option<Present>,
    input: Mapper,
    frames: u32,
    frame_times: Vec<f32>,
    window_start: Instant,
    present_enabled: bool,
    /// Opened hidden on purpose (timing and test probes): always rendered.
    hidden: bool,
    /// Probe override of `Window::presentable` (None: ask the window).
    presentable_override: Option<bool>,
    /// This frame's 2D prepare found the window unable to show it, or its last copy
    /// still running: its present skips too (`ready_2d`).
    skip_2d: bool,
}

/// A one-box frame that creates the renderer and warms its pipelines before Play.
pub fn warm_frame() -> RenderFrame {
    let mut warm = RenderFrame::default();
    warm.environment = Environment::default();
    warm.draws.push(Draw {
        entity: 1,
        address: "warm".into(),
        label: "warm".into(),
        geometry: Geometry::Shape {
            shape: MeshShape::Box,
            size: [1.0; 3],
        },
        world: glam::Mat4::IDENTITY,
        color: [1.0; 4],
        roughness: 0.5,
        metallic: 0.0,
        emissive: [0.0; 3],
        emissive_factor: None,
        material: None,
        skin: None,
    });
    warm
}

impl Windowed {
    pub(crate) fn raw_events(&mut self) -> Result<Vec<Event>> {
        self.window.poll()
    }
    pub(crate) fn ui_text_input(&self, enabled: bool) -> Result<()> {
        self.window.set_text_input(enabled)
    }
    pub(crate) fn window_size(&self) -> Result<[u32; 2]> {
        self.window.size()
    }
    /// Physical pixels per window point: the game UI's scale, so its layout units are
    /// the window coordinates SDL reports the pointer in.
    pub(crate) fn pixel_density(&self) -> f32 {
        self.window.pixel_density()
    }
    pub(crate) fn set_window_mode(
        &self,
        mode: ara_platform::GameWindowMode,
        size: [u32; 2],
    ) -> Result<()> {
        self.window.set_game_mode(mode, size)
    }
    /// Startup-only diagnostic: render normally without creating a swapchain.
    pub(crate) fn disable_presentation(&mut self) {
        debug_assert!(self.present.is_none() && self.present2d.is_none());
        self.present_enabled = false;
    }
    /// Updates startup status without changing the presentation surface or inputs.
    pub(crate) fn title(&self, text: &str) -> Result<()> {
        self.window.set_title(text)
    }

    /// Loading presentations are not game-frame timing samples.
    pub(crate) fn begin_game(&mut self) {
        self.frames = 0;
        self.frame_times.clear();
        self.window_start = Instant::now();
    }

    /// Opens the window and warms the GPU device and pipelines with a one-box frame,
    /// so a pooled player pays none of that after Play.
    pub fn open(
        name: &str,
        size: [u32; 2],
        cache: &Path,
        quality: ara_runtime_render::QualityChoice,
        options: ara_runtime_render::RenderOptions,
    ) -> Result<(Self, Presenter)> {
        Self::open_with_visibility(name, size, cache, quality, options, false)
    }

    /// Runs the same render/present loop on an SDL window hidden at creation.
    /// Timing probes never flash a window or steal focus from the owner's desktop.
    pub fn open_offscreen(
        name: &str,
        size: [u32; 2],
        cache: &Path,
        quality: ara_runtime_render::QualityChoice,
        options: ara_runtime_render::RenderOptions,
    ) -> Result<(Self, Presenter)> {
        Self::open_with_visibility(name, size, cache, quality, options, true)
    }

    fn open_with_visibility(
        name: &str,
        size: [u32; 2],
        cache: &Path,
        quality: ara_runtime_render::QualityChoice,
        options: ara_runtime_render::RenderOptions,
        offscreen: bool,
    ) -> Result<(Self, Presenter)> {
        ara_gpu::cpu_scope!("Player.device_and_warm_pipelines");
        let (window, extensions, size) = Self::open_unprepared(name, size, offscreen)?;
        let mut presenter = Presenter::new(size, extensions, cache);
        presenter.set_quality(quality);
        presenter.set_render_options(options);
        presenter.prepare(&warm_frame())?;
        Ok((window, presenter))
    }

    /// Drains window events into `player`'s action injections for the next tick.
    /// Returns true when the user closed the window (Esc or the close button).
    pub fn events(&mut self, player: PlayerId) -> Result<(bool, Vec<Injection>)> {
        let quit = self.poll_input()?;
        Ok((quit, self.input.injections(player)))
    }

    pub(crate) fn view_events(
        &mut self,
        player: PlayerId,
        spectator: bool,
    ) -> Result<(bool, Vec<Injection>, Option<crate::input::Navigation>)> {
        let quit = self.poll_input()?;
        let (inputs, navigation) = self.input.for_view(player, spectator);
        Ok((quit, inputs, navigation))
    }

    fn poll_input(&mut self) -> Result<bool> {
        let mut quit = false;
        for event in self.window.poll()? {
            match event {
                Event::Quit
                | Event::Key {
                    scancode: ESCAPE,
                    pressed: true,
                    ..
                } => quit = true,
                Event::Key {
                    scancode,
                    pressed,
                    repeat: false,
                } => self.input.apply(DeviceEvent::Key {
                    scancode,
                    down: pressed,
                }),
                Event::FocusLost => self.input.apply(DeviceEvent::FocusLost),
                Event::MouseButton { button, pressed } => {
                    self.input.apply(DeviceEvent::MouseButton {
                        button,
                        down: pressed,
                    });
                }
                Event::MouseMotion { delta, .. } => self.input.apply(DeviceEvent::MouseMotion {
                    dx: delta[0],
                    dy: delta[1],
                }),
                _ => {}
            }
        }
        Ok(quit)
    }

    /// Renders and presents one extracted frame, recreating the swapchain on resize.
    /// Touches no simulation state, so it overlaps the frame's audio worker.
    pub fn present(
        &mut self,
        presenter: &mut Presenter,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<ara_runtime_render::fidelity::native::Content>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Player.window_present");
        let started = Instant::now();
        let size = self.window.size()?;
        if size.contains(&0) {
            std::thread::sleep(Duration::from_millis(5));
            return Ok(());
        }
        // Minimized or occluded, nothing would show: render and present nothing (a
        // driver's present path may stall until the window shows) and keep pumping.
        if !self.presentable() {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(());
        }
        if presenter.size() != size {
            self.recreate(presenter)?;
            presenter.resize(size)?;
        }
        // Frames in flight: the scene, its UI composite and the window blit return
        // once submitted; each waits its previous frame at the start of the next, so
        // simulation and extraction overlap the GPU. Everything after the scene here
        // is GPU work on the same queue.
        presenter.set_frames_in_flight(true)?;
        // The UI composite and the window copy of the frame in flight may still read
        // the image this frame's scene rewrites; on Metal they run on queues of their
        // own, so the scene waits for them on the GPU.
        if let Some(renderer) = presenter.renderer_mut() {
            if let Some(surface) = &self.game_ui {
                surface.order_before(renderer.gpu_mut());
            }
            if let Some(present) = &self.present {
                present.order_before(renderer.gpu_mut());
            }
        }
        presenter.present_with(frame, resolve)?;
        // The frame in flight is stalled (Presenter::STALL_WAIT): nothing new to show.
        if presenter.stats().skipped {
            return Ok(());
        }
        let cpu = started.elapsed() - presenter.stats().render;
        let renderer = presenter
            .renderer()
            .ok_or_else(|| anyhow::anyhow!("renderer missing"))?;
        if self.present_enabled && self.present.is_none() {
            // SAFETY: the window and device both outlive `present`, which is destroyed
            // after device idle in `recreate`/`Drop` ordering below.
            self.present = Some(unsafe { Present::create(renderer.gpu(), &self.window)? });
        }
        // The window copy returns once submitted, like the scene and its UI composite.
        // Set every frame: a presentation adopted from the loading view
        // (`finish_loading`) starts synchronous, and its copy would wait for this
        // frame's GPU work before the next frame could record.
        if let Some(present) = &mut self.present {
            // SAFETY: created on this renderer's device, or on the loading device that
            // shares its native device; both are live.
            unsafe { present.set_frames_in_flight(renderer.gpu(), true)? };
        }
        // The game UI (if any) composites the scene into its own target and presents it.
        if let Some(surface) = &mut self.game_ui {
            surface.set_frames_in_flight(true)?;
            if !surface.render(presenter)? {
                return Ok(());
            }
        }
        let result = match &mut self.present {
            // SAFETY: the renderer, or the game UI over it, submitted its CopySource image
            // ahead of the blit on this queue; both owners are live.
            Some(present) => unsafe {
                match &self.game_ui {
                    Some(surface) => surface.present(renderer.gpu(), present),
                    None => present.present_image(
                        renderer.gpu(),
                        presenter.output().context("presented image missing")?,
                        presenter.output_size(),
                    ),
                }
            },
            None => Ok(()),
        };
        if let Err(error) = result {
            if error.downcast_ref::<ara_gpu::vk::Result>()
                == Some(&ara_gpu::vk::Result::ERROR_OUT_OF_DATE_KHR)
            {
                self.recreate(presenter)?;
            } else {
                return Err(error);
            }
        }
        self.frames += 1;
        self.frame_times
            .push((cpu + presenter.stats().render).as_secs_f32() * 1000.0);
        Ok(())
    }

    /// Whether presenting can show anything: hidden probe windows always render; a
    /// minimized or occluded window does not (`Window::presentable`); probes override.
    fn presentable(&self) -> bool {
        self.presentable_override
            .unwrap_or_else(|| self.hidden || self.window.presentable())
    }

    /// Probes (`standalone/probes.rs`) force `presentable` (Some) or return it to the
    /// window (None), standing in for a minimized window without touching the desktop.
    pub(crate) fn override_presentable(&mut self, presentable: Option<bool>) {
        self.presentable_override = presentable;
    }

    /// Prepares a 2D extraction before the frame's read-only audio worker starts.
    pub fn prepare_2d(
        &mut self,
        presenter: &mut Presenter2d,
        world: &mut ara_sim::bevy_ecs::world::World,
        tick: u64,
    ) -> Result<()> {
        let size = self.window.size()?;
        if size.contains(&0) {
            return Ok(());
        }
        self.skip_2d = !self.ready_2d(presenter)?;
        if self.skip_2d {
            return Ok(());
        }
        if presenter.renderer().is_some_and(|r| r.size() != size) {
            self.recreate_2d(presenter)?;
        }
        presenter.prepare(world, tick, size, &self.window.extensions()?)
    }

    /// Whether a 2D frame may upload and render now. The 2D renderer waits for its
    /// own submissions, so it must never queue them behind a stalled presentation:
    /// nothing while the window cannot show (as `present`), and nothing until its
    /// last copy has finished, waiting at most `Presenter::STALL_WAIT`.
    fn ready_2d(&mut self, presenter: &Presenter2d) -> Result<bool> {
        if !self.presentable() {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(false);
        }
        let (Some(present), Some(renderer)) = (&mut self.present2d, presenter.renderer()) else {
            return Ok(true);
        };
        // SAFETY: created on this 2D renderer's live device.
        unsafe { present.settled(renderer.gpu(), Presenter::STALL_WAIT) }
    }

    /// Renders and blits the native 2D target; no pixel data crosses the CPU. Skips
    /// the frame its prepare skipped (`ready_2d`).
    pub fn present_2d(&mut self, presenter: &mut Presenter2d) -> Result<()> {
        if std::mem::take(&mut self.skip_2d) {
            return Ok(());
        }
        if self.window.size()?.contains(&0) {
            std::thread::sleep(Duration::from_millis(5));
            return Ok(());
        }
        let started = Instant::now();
        presenter.present()?;
        let renderer = presenter.renderer().context("2D renderer missing")?;
        if self.present_enabled && self.present2d.is_none() {
            // SAFETY: the window and 2D device outlive this native presentation object.
            self.present2d = Some(unsafe { Present::create(renderer.gpu(), &self.window)? });
        }
        let image = renderer.output();
        let result = match &mut self.present2d {
            // SAFETY: this completed image is CopySource and both owners are live.
            Some(present) => unsafe {
                present.present_image(renderer.gpu(), image, renderer.size())
            },
            None => Ok(()),
        };
        if let Err(error) = result {
            if error.downcast_ref::<ara_gpu::vk::Result>()
                == Some(&ara_gpu::vk::Result::ERROR_OUT_OF_DATE_KHR)
            {
                self.recreate_2d(presenter)?;
            } else {
                return Err(error);
            }
        }
        self.frames += 1;
        self.frame_times
            .push(started.elapsed().as_secs_f32() * 1000.0);
        Ok(())
    }

    /// Releases the 2D swapchain while its device is still live.
    pub fn shutdown_2d(&mut self, presenter: &Presenter2d) -> Result<()> {
        self.recreate_2d(presenter)
    }

    fn recreate_2d(&mut self, presenter: &Presenter2d) -> Result<()> {
        if let (Some(mut present), Some(renderer)) = (self.present2d.take(), presenter.renderer()) {
            // SAFETY: the native device is idle before its presentation objects retire.
            unsafe {
                renderer.gpu().wait_idle()?;
                present.retire(renderer.gpu());
            }
        }
        Ok(())
    }

    /// Destroys presentation objects after the device is idle (before exit).
    pub fn shutdown(&mut self, presenter: &Presenter) -> Result<()> {
        self.game_ui = None;
        self.recreate(presenter)
    }

    fn recreate(&mut self, presenter: &Presenter) -> Result<()> {
        if let (Some(mut present), Some(renderer)) = (self.present.take(), presenter.renderer()) {
            // SAFETY: waiting for device idle before destroying presentation objects.
            unsafe {
                renderer.gpu().wait_idle()?;
                present.retire(renderer.gpu());
            }
        }
        Ok(())
    }

    /// Frames per second and p95 frame time since the previous call.
    pub fn take_stats(&mut self) -> (f64, f64) {
        let stats = frame_stats(self.frames, self.window_start, &mut self.frame_times);
        self.frames = 0;
        self.window_start = Instant::now();
        stats
    }
}

/// Frames per second since `since` and the p95 of `times` (milliseconds), which it
/// drains.
pub fn frame_stats(frames: u32, since: Instant, times: &mut Vec<f32>) -> (f64, f64) {
    let seconds = since.elapsed().as_secs_f64().max(1e-6);
    let fps = f64::from(frames) / seconds;
    let mut times = std::mem::take(times);
    times.sort_by(f32::total_cmp);
    let p95 = times
        .get(((times.len() as f64 * 0.95) as usize).min(times.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    (fps, f64::from(p95))
}
