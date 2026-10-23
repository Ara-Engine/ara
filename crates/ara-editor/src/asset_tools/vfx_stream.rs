//! Fixed-tick GPU preview on its own worker/device. Presentation consumes owned pixels.
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};
struct Request {
    effect: Option<ara_vfx::Effect>,
    playing: bool,
}
pub(crate) struct VfxStream {
    send: Sender<Request>,
    receive: Receiver<Result<image::RgbaImage, String>>,
    identity: Option<String>,
    playing: bool,
}
impl VfxStream {
    pub fn new(root: &Path) -> Self {
        let root = root.to_owned();
        let (send, requests) = mpsc::channel::<Request>();
        let (frames, receive) = mpsc::sync_channel(2);
        std::thread::spawn(move || {
            let mut gpu = None::<ara_gpu::vulkan::VulkanDevice>;
            let mut system = None::<ara_vfx_gpu::GpuSystem>;
            let mut playing = false;
            let mut hz = 60;
            let mut remainder = 0;
            let view = ara_vfx_gpu::View::default();
            let budget = Duration::from_secs_f64(1. / 30.);
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
                if let Some(request) = request {
                    playing = request.playing;
                    if let Some(effect) = request.effect {
                        let loaded = (|| -> anyhow::Result<()> {
                            let cooked = super::vfx_preview::cooked(&root, &effect)?;
                            if gpu.is_none() {
                                gpu = Some(ara_gpu::vulkan::VulkanDevice::new(
                                    &[],
                                    root.join(".ara/cache/vfx-live-gpu"),
                                )?);
                            }
                            let device = gpu
                                .as_mut()
                                .ok_or_else(|| anyhow::anyhow!("preview device missing"))?;
                            let replacement =
                                ara_vfx_gpu::GpuSystem::new(device, &effect, &cooked, [384, 384])?;
                            if let Some(old) = system.replace(replacement) {
                                old.retire(device)?;
                            }
                            hz = effect.hz;
                            remainder = 0;
                            if let Some(system) = &mut system {
                                for _ in 0..60 {
                                    system.step(device, &view)?;
                                }
                            }
                            Ok(())
                        })();
                        if let Err(error) = loaded {
                            let _ = frames.try_send(Err(error.to_string()));
                        }
                    }
                }
                if let (Some(device), Some(system)) = (&mut gpu, &mut system) {
                    let rendered = (|| -> anyhow::Result<image::RgbaImage> {
                        if playing {
                            remainder += hz;
                            let ticks = remainder / 30;
                            remainder %= 30;
                            for _ in 0..ticks {
                                system.step(device, &view)?;
                            }
                        }
                        system.render(device, &view)?;
                        let image =
                            image::RgbaImage::from_raw(384, 384, system.read_output(device)?)
                                .ok_or_else(|| anyhow::anyhow!("invalid preview pixels"))?;
                        device.context().assert_clean()?;
                        Ok(image)
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
        }
    }
    pub fn show(&mut self, effect: ara_vfx::Effect, playing: bool) {
        let identity = effect.content_hash().ok();
        if self.identity != identity {
            self.identity = identity;
            self.playing = playing;
            let _ = self.send.send(Request {
                effect: Some(effect),
                playing,
            });
        } else if self.playing != playing {
            self.playing = playing;
            let _ = self.send.send(Request {
                effect: None,
                playing,
            });
        }
    }
    pub fn pause(&mut self) {
        if self.playing {
            self.playing = false;
            let _ = self.send.send(Request {
                effect: None,
                playing: false,
            });
        }
    }
    pub fn poll(&mut self) -> impl Iterator<Item = Result<image::RgbaImage, String>> + '_ {
        self.receive.try_iter().take(2)
    }
}
