//! Fixed-tick GPU preview on its own worker/device. Presentation consumes owned pixels.
//! The VFX tool's timeline drives it: play/pause, a rate (25-400 %), restart from
//! tick 0 and single steps while paused. Each frame carries the GPU counters.
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

/// Preview frames per second; the effect's own rate sets the ticks per frame.
const FPS: u32 = 30;

#[derive(Default)]
struct Request {
    effect: Option<ara_vfx::Effect>,
    playing: bool,
    /// Simulation rate in percent of real time.
    rate: u32,
    /// Rebuild the system from the last effect and start again at tick 0.
    restart: bool,
    /// Ticks to run now (stepping while paused).
    step: u32,
}

/// One rendered preview frame and the simulation's counters after it.
pub(crate) struct Frame {
    pub image: image::RgbaImage,
    /// Completed simulation ticks since the (re)start.
    pub tick: u32,
    /// Live particles, all emitters.
    pub alive: u32,
    /// Live particles per emitter, by name.
    pub emitters: Vec<(String, u32)>,
}

pub(crate) struct VfxStream {
    send: Sender<Request>,
    receive: Receiver<Result<Frame, String>>,
    identity: Option<String>,
    playing: bool,
    rate: u32,
}
impl VfxStream {
    pub fn new(root: &Path) -> Self {
        let root = root.to_owned();
        let (send, requests) = mpsc::channel::<Request>();
        let (frames, receive) = mpsc::sync_channel(2);
        std::thread::spawn(move || {
            let mut gpu = None::<ara_gpu::vulkan::VulkanDevice>;
            let mut system = None::<ara_vfx_gpu::GpuSystem>;
            let mut effect = None::<ara_vfx::Effect>;
            let mut playing = false;
            let mut rate = 100;
            let mut remainder = 0;
            let mut view = ara_vfx_gpu::View::default();
            let budget = Duration::from_secs_f64(1. / f64::from(FPS));
            loop {
                let start = Instant::now();
                let request = if playing {
                    match requests.recv_timeout(budget) {
                        Ok(r) => Some(r),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(_) => break,
                    }
                } else {
                    match requests.recv() {
                        Ok(r) => Some(r),
                        Err(_) => break,
                    }
                };
                let mut step = 0;
                if let Some(request) = request {
                    playing = request.playing;
                    rate = request.rate.clamp(25, 400);
                    step = request.step;
                    // A new effect warms up for a second so edits show a steady state;
                    // a restart starts from tick 0.
                    let load = match request.effect {
                        Some(new) => Some((new, 60)),
                        None if request.restart => effect.clone().map(|e| (e, 0)),
                        None => None,
                    };
                    if let Some((new, warm)) = load {
                        let loaded = (|| -> anyhow::Result<()> {
                            let cooked = super::vfx_preview::cooked(&root, &new)?;
                            if gpu.is_none() {
                                gpu = Some(ara_gpu::vulkan::VulkanDevice::new(
                                    &[],
                                    root.join(".ara/cache/vfx-live-gpu"),
                                )?);
                            }
                            let device = gpu
                                .as_mut()
                                .ok_or_else(|| anyhow::anyhow!("preview device missing"))?;
                            let mut replacement =
                                ara_vfx_gpu::GpuSystem::new(device, &new, &cooked, [768, 768])?;
                            for _ in 0..warm {
                                replacement.step(device, &view)?;
                            }
                            // Frame what the particles do, not the loose culling bounds:
                            // once per effect, so edits never move the camera.
                            if effect.as_ref().is_none_or(|old| old.name != new.name) {
                                view.camera = super::vfx_preview::framing(&new);
                            }
                            if let Some(old) = system.replace(replacement) {
                                old.retire(device)?;
                            }
                            remainder = 0;
                            effect = Some(new);
                            Ok(())
                        })();
                        if let Err(error) = loaded {
                            let _ = frames.try_send(Err(error.to_string()));
                        }
                    }
                }
                if let (Some(device), Some(system), Some(effect)) = (&mut gpu, &mut system, &effect)
                {
                    let rendered = (|| -> anyhow::Result<Frame> {
                        let mut ticks = step;
                        if playing {
                            remainder += effect.hz * rate;
                            ticks += remainder / (FPS * 100);
                            remainder %= FPS * 100;
                        }
                        for _ in 0..ticks {
                            system.step(device, &view)?;
                        }
                        system.render(device, &view)?;
                        let image =
                            image::RgbaImage::from_raw(768, 768, system.read_output(device)?)
                                .ok_or_else(|| anyhow::anyhow!("invalid preview pixels"))?;
                        // Counters exist once the first tick has run.
                        let (tick, alive, emitters) = match system.stats(device) {
                            Ok(stats) => (
                                stats.tick,
                                stats.particles_alive,
                                stats
                                    .emitters
                                    .into_iter()
                                    .map(|e| (e.name, e.alive))
                                    .collect(),
                            ),
                            Err(_) => (0, 0, Vec::new()),
                        };
                        device.context().assert_clean()?;
                        Ok(Frame {
                            image,
                            tick,
                            alive,
                            emitters,
                        })
                    })()
                    .map_err(|e| e.to_string());
                    let _ = frames.try_send(rendered);
                }
                if playing && let Some(wait) = budget.checked_sub(start.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            if let (Some(device), Some(system)) = (&mut gpu, system) {
                let _ = system.retire(device);
            }
        });
        Self {
            send,
            receive,
            identity: None,
            playing: false,
            rate: 100,
        }
    }
    /// Shows `effect` (reloading it when its content changed) at `rate` percent.
    pub fn show(&mut self, effect: ara_vfx::Effect, playing: bool, rate: u32) {
        let identity = effect.content_hash().ok();
        if self.identity != identity {
            self.identity = identity;
            self.playing = playing;
            self.rate = rate;
            let _ = self.send.send(Request {
                effect: Some(effect),
                playing,
                rate,
                ..Request::default()
            });
        } else if self.playing != playing || self.rate != rate {
            self.playing = playing;
            self.rate = rate;
            self.transport(false, 0);
        }
    }
    /// Starts the shown effect again from tick 0.
    pub fn restart(&mut self) {
        self.transport(true, 0);
    }
    /// Runs one tick (the timeline's Step, while paused).
    pub fn step(&mut self) {
        self.transport(false, 1);
    }
    pub fn pause(&mut self) {
        if self.playing {
            self.playing = false;
            self.transport(false, 0);
        }
    }
    fn transport(&mut self, restart: bool, step: u32) {
        let _ = self.send.send(Request {
            effect: None,
            playing: self.playing,
            rate: self.rate,
            restart,
            step,
        });
    }
    pub fn poll(&mut self) -> impl Iterator<Item = Result<Frame, String>> + '_ {
        self.receive.try_iter().take(2)
    }
}
