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

/// SDL scancode of Escape: closes a windowed player.
const ESCAPE: i32 = 41;

/// The player's window and device-input state.
pub struct Windowed {
    window: Window,
    present: Option<Present>,
    present2d: Option<Present>,
    input: Mapper,
    frames: u32,
    frame_times: Vec<f32>,
    window_start: Instant,
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
        skin: None,
    });
    warm
}

impl Windowed {
    /// Opens the window and warms the GPU device and pipelines with a one-box frame,
    /// so a pooled player pays none of that after Play.
    pub fn open(name: &str, size: [u32; 2], cache: &Path) -> Result<(Self, Presenter)> {
        let window = Window::new(&format!("Ara | {name} | waiting for Play"), size)?;
        let mut presenter = Presenter::new(window.size()?, window.extensions()?, cache);
        presenter.prepare(&warm_frame())?;
        Ok((
            Self {
                window,
                present: None,
                present2d: None,
                input: Mapper::default(),
                frames: 0,
                frame_times: Vec::new(),
                window_start: Instant::now(),
            },
            presenter,
        ))
    }

    /// Drains window events into `player`'s action injections for the next tick.
    /// Returns true when the user closed the window (Esc or the close button).
    pub fn events(&mut self, player: PlayerId) -> Result<(bool, Vec<Injection>)> {
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
        Ok((quit, self.input.injections(player)))
    }

    /// Renders and presents one extracted frame, recreating the swapchain on resize.
    /// Touches no simulation state, so it overlaps the frame's audio worker.
    pub fn present(&mut self, presenter: &mut Presenter, frame: &RenderFrame) -> Result<()> {
        let started = Instant::now();
        let size = self.window.size()?;
        if size.contains(&0) {
            std::thread::sleep(Duration::from_millis(5));
            return Ok(());
        }
        if presenter.renderer().is_some_and(|r| r.size() != size) {
            self.recreate(presenter)?;
            presenter.resize(size)?;
        }
        presenter.present(frame)?;
        let cpu = started.elapsed() - presenter.stats().render;
        let renderer = presenter
            .renderer()
            .ok_or_else(|| anyhow::anyhow!("renderer missing"))?;
        if self.present.is_none() {
            // SAFETY: the window and device both outlive `present`, which is destroyed
            // after device idle in `recreate`/`Drop` ordering below.
            self.present = Some(unsafe { Present::new(renderer.gpu().context(), &self.window)? });
        }
        let image = renderer
            .gpu()
            .native_image(presenter.output().context("presented image missing")?)?;
        let result = match &mut self.present {
            // SAFETY: the renderer completed this CopySource image; both owners are live.
            Some(present) => unsafe {
                present.display(renderer.gpu().context(), image, renderer.size())
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
        if presenter.renderer().is_some_and(|r| r.size() != size) {
            self.recreate_2d(presenter)?;
        }
        presenter.prepare(world, tick, size, &self.window.extensions()?)
    }

    /// Renders and blits the native 2D target; no pixel data crosses the CPU.
    pub fn present_2d(&mut self, presenter: &mut Presenter2d) -> Result<()> {
        if self.window.size()?.contains(&0) {
            std::thread::sleep(Duration::from_millis(5));
            return Ok(());
        }
        let started = Instant::now();
        presenter.present()?;
        let renderer = presenter.renderer().context("2D renderer missing")?;
        if self.present2d.is_none() {
            // SAFETY: the window and 2D device outlive this native presentation object.
            self.present2d = Some(unsafe { Present::new(renderer.gpu().context(), &self.window)? });
        }
        let image = renderer.gpu().native_image(renderer.output())?;
        let result = match &mut self.present2d {
            // SAFETY: this completed image is CopySource and both owners are live.
            Some(present) => unsafe {
                present.display(renderer.gpu().context(), image, renderer.size())
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
                renderer.gpu().context().device.device_wait_idle()?;
                present.destroy(renderer.gpu().context());
            }
        }
        Ok(())
    }

    /// Destroys presentation objects after the device is idle (before exit).
    pub fn shutdown(&mut self, presenter: &Presenter) -> Result<()> {
        self.recreate(presenter)
    }

    fn recreate(&mut self, presenter: &Presenter) -> Result<()> {
        if let (Some(mut present), Some(renderer)) = (self.present.take(), presenter.renderer()) {
            // SAFETY: waiting for device idle before destroying presentation objects.
            unsafe {
                renderer.gpu().context().device.device_wait_idle()?;
                present.destroy(renderer.gpu().context());
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
