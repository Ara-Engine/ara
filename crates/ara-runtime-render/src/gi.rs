//! Probe GI acquisition for the raster path: the frame's native content is
//! ray-traced once by a temporary fidelity view (`bake_probe_surfaces`), and the
//! raster renderer relights the result every frame without rays.
//!
//! [`Acquisition`] runs bakes on a worker thread with its own ray-tracing device
//! (the presentation device may be raster-only) and caches results on disk by
//! content key, so loading and the frame thread never wait for ray tracing.
use crate::fidelity::native::{self, Content, NativeConfig};
use anyhow::{Context, Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::look::gi::{ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS};
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler,
    environment::FrameEnvironment,
};
use ara_runtime::extract::RenderFrame;
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
};

/// Bumped whenever the bake or the cache layout changes.
const VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"ARAGI\0\0\x01";

/// Residency used only while baking (full-resolution albedo for the caches).
fn config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1024 * 1024 * 1024,
        upload_bytes: 64 * 1024 * 1024,
    }
}

/// Background, disk-cached probe surface acquisition for one presenter.
pub(crate) struct Acquisition {
    cache: PathBuf,
    /// Content key of the newest request.
    key: Option<u64>,
    pending: Option<Receiver<Result<ProbeSurfaces>>>,
}

impl Acquisition {
    pub(crate) fn new(cache: PathBuf) -> Self {
        Self {
            cache: cache.join("probe-gi"),
            key: None,
            pending: None,
        }
    }

    /// Starts acquiring `frame`'s content unless `key` was already requested.
    /// `content` is only resolved for a new key.
    pub(crate) fn request(
        &mut self,
        key: u64,
        frame: &RenderFrame,
        content: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        let key = key ^ u64::from(VERSION);
        if self.key == Some(key) {
            return Ok(());
        }
        self.key = Some(key);
        let snapshot = snapshot(frame);
        let content = content()?;
        let cache = self.cache.clone();
        let (send, receive) = channel();
        std::thread::Builder::new()
            .name("probe-gi".into())
            .spawn(move || {
                let _ = send.send(acquire(&cache, key, &snapshot, content));
            })
            .context("starting the probe GI worker")?;
        self.pending = Some(receive);
        Ok(())
    }

    /// The finished acquisition, once; `wait` blocks until it completes.
    /// Failures are reported and leave the scene with sky-only ambient light.
    pub(crate) fn poll(&mut self, wait: bool) -> Option<ProbeSurfaces> {
        let receive = self.pending.as_ref()?;
        let result = if wait {
            receive.recv().ok()
        } else {
            match receive.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => None,
            }
        };
        self.pending = None;
        match result {
            Some(Ok(surfaces)) => Some(surfaces),
            Some(Err(error)) => {
                eprintln!("ara-runtime-render: probe GI acquisition failed: {error:#}");
                None
            }
            None => {
                eprintln!("ara-runtime-render: probe GI worker stopped");
                None
            }
        }
    }
}

/// The frame fields a bake reads (draws, terrain, lights, look, camera).
fn snapshot(frame: &RenderFrame) -> RenderFrame {
    let mut copy = RenderFrame::default();
    copy.tick = frame.tick;
    copy.camera = frame.camera;
    copy.environment = frame.environment;
    copy.look = frame.look;
    copy.sun = frame.sun;
    copy.lights = frame.lights.clone();
    copy.draws = frame.draws.clone();
    copy.palettes = frame.palettes.clone();
    copy.terrain = frame.terrain.clone();
    copy
}

/// Loads `key` from the cache, or bakes it on a new ray-tracing device and stores it.
fn acquire(
    cache: &std::path::Path,
    key: u64,
    frame: &RenderFrame,
    content: Content,
) -> Result<ProbeSurfaces> {
    let path = cache.join(format!("{key:016x}.gi"));
    if let Ok(bytes) = std::fs::read(&path) {
        match decode(&bytes) {
            Ok(surfaces) => return Ok(surfaces),
            Err(error) => eprintln!(
                "ara-runtime-render: ignoring probe GI cache {}: {error:#}",
                path.display()
            ),
        }
    }
    let mut gpu = VulkanDevice::new(&[], cache.join("device"))?;
    ensure!(
        gpu.context().info.ray_tracing,
        "probe GI acquisition needs ray queries (raster cube capture is not built yet)"
    );
    let surfaces = bake(&mut gpu, frame, content)?;
    std::fs::create_dir_all(cache)?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, encode(&surfaces))?;
    std::fs::rename(&temporary, &path)?;
    Ok(surfaces)
}

fn encode(s: &ProbeSurfaces) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    for v in s.origin.iter().chain(&s.spacing) {
        bytes.extend(v.to_le_bytes());
    }
    for c in s.counts {
        bytes.extend(c.to_le_bytes());
    }
    bytes.extend_from_slice(bytemuck::cast_slice(&s.texels));
    bytes.extend_from_slice(bytemuck::cast_slice(&s.statics));
    bytes
}

fn decode(bytes: &[u8]) -> Result<ProbeSurfaces> {
    ensure!(
        bytes.len() >= 44 && &bytes[..8] == MAGIC,
        "not a probe GI cache"
    );
    let word = |i: usize| -> [u8; 4] {
        [
            bytes[8 + i * 4],
            bytes[9 + i * 4],
            bytes[10 + i * 4],
            bytes[11 + i * 4],
        ]
    };
    let float = |i| f32::from_le_bytes(word(i));
    let counts = [6, 7, 8].map(|i| u32::from_le_bytes(word(i)));
    let probes = counts.iter().map(|&c| c as usize).product::<usize>();
    let texel_bytes = probes * SURFACE_TEXELS * 16;
    ensure!(
        bytes.len() == 44 + texel_bytes + probes * STATIC_VECTORS * 16,
        "truncated probe GI cache"
    );
    let surfaces = ProbeSurfaces {
        origin: [float(0), float(1), float(2)],
        spacing: [float(3), float(4), float(5)],
        counts,
        texels: bytemuck::pod_collect_to_vec(&bytes[44..44 + texel_bytes]),
        statics: bytemuck::pod_collect_to_vec(&bytes[44 + texel_bytes..]),
    };
    surfaces.validate()?;
    Ok(surfaces)
}

/// Bakes the probe surfaces of `frame`'s content on a ray-tracing device.
fn bake(gpu: &mut VulkanDevice, frame: &RenderFrame, content: Content) -> Result<ProbeSurfaces> {
    let started = std::time::Instant::now();
    let prepared = native::prepare(frame, content, config())?;
    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
    let mut view = FidelityView::new_native(
        gpu,
        &prepared.scene,
        prepared.textures.fork_sources(config())?,
        Settings {
            render_size: [64, 36],
            display_size: [64, 36],
            tier: Tier::Rt,
            mode: Mode::Hybrid,
            upscaler: Upscaler::Taau,
            probe_ray_budget: 64,
            reflection_cutoff: 1.,
            post: look.post(PostSettings::default()),
        },
        None,
    )?;
    let surfaces = (|| {
        view.set_environment(&look)?;
        for _ in 0..256 {
            if view.refine_native_textures(gpu)?.pending_textures == 0 {
                break;
            }
        }
        let camera = crate::camera(frame)?;
        view.render(
            gpu,
            &FrameInput {
                camera: &camera,
                tick: 0,
                delta_seconds: 1. / 60.,
                reset: true,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        view.bake_probe_surfaces(gpu)
    })();
    view.destroy(gpu)?;
    let surfaces = surfaces?;
    ensure!(!surfaces.is_empty(), "empty probe bake");
    eprintln!(
        "ara-runtime-render: baked {} GI probes ({:?}) in {:.1} s",
        surfaces.len(),
        surfaces.counts,
        started.elapsed().as_secs_f64()
    );
    Ok(surfaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trips_and_rejects_truncation() -> Result<()> {
        let surfaces = ProbeSurfaces {
            origin: [1., 2., 3.],
            spacing: [0.5, 1., 2.],
            counts: [2, 2, 2],
            texels: (0..8 * SURFACE_TEXELS as u32)
                .map(|i| [i, i + 1, i + 2, i + 3])
                .collect(),
            statics: vec![[0.25, 1., 0., 1.]; 8 * STATIC_VECTORS],
        };
        let bytes = encode(&surfaces);
        assert_eq!(decode(&bytes)?, surfaces);
        assert!(decode(&bytes[..bytes.len() - 4]).is_err());
        Ok(())
    }
}
