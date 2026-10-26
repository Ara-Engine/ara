//! GPU particle effects (ara-vfx-gpu) composited into 3D captures. Each
//! `VisualEffect` replays its cooked library effect to the capture tick over the
//! renderer's completed frame (HDR background, depth for occlusion and soft particles),
//! with the camera shifted into the effect's local space so depth stays consistent.
//! Presentation only: gameplay never reads GPU particles.
use anyhow::{Context, Result};
use ara_render::{
    Renderer,
    capture::{Capture, CaptureRequest},
    scene::CameraSpec,
};
use ara_runtime::extract::RenderFrame;
use ara_vfx::Effect;
use ara_vfx_gpu::{GpuSystem, Lighting, Placement, SceneColor, SceneDepth, View};
use std::{collections::BTreeMap, path::PathBuf};

/// Ticks replayed at most per capture (effects loop well within it).
const MAX_REPLAY: u64 = 240;

mod preparation;
#[cfg(test)]
mod tests;

/// The cooked library effects shipped with ara-vfx-gpu (`cooked/<name>`); a project
/// asset pipeline for authored effects is the next step.
fn cooked_root() -> PathBuf {
    ara_gpu::cooked::resource_dir(
        "ara-vfx-gpu",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../ara-vfx-gpu/cooked"),
    )
}

fn library(name: &str) -> Result<Effect> {
    ara_vfx::samples::find(name).with_context(|| format!("no cooked VFX effect `{name}`"))
}

/// Effect systems on the renderer's device, by effect name and capture size.
#[derive(Default)]
pub struct Effects {
    systems: BTreeMap<(String, [u32; 2]), GpuSystem>,
    live: BTreeMap<u64, Live>,
    failed: BTreeMap<u64, String>,
    pending: BTreeMap<u64, preparation::Pending>,
    gpu_ms: f64,
}

struct Live {
    name: String,
    size: [u32; 2],
    system: GpuSystem,
    tick: u64,
    rendered: bool,
    warm_remaining: u32,
}

impl Effects {
    /// All particle simulation/draw timestamps in the last live composite.
    pub fn gpu_ms(&self) -> f64 {
        self.gpu_ms
    }
    /// Live effects whose PSOs are still compiling. No GPU wait is performed.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty() || self.live.values().any(|live| live.warm_remaining > 0)
    }
    /// Advances persistent per-entity particle systems to this presentation tick,
    /// then redraws them over the current camera/background. No pixel readbacks.
    pub fn present(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mut color: SceneColor,
        depth: SceneDepth,
    ) -> Result<SceneColor> {
        self.gpu_ms = 0.;
        // Particles draw over the scene's HDR, at the render size.
        self.prepare_systems(renderer, frame)?;
        for effect in &frame.effects {
            if self.failed.get(&effect.entity) == Some(&effect.effect) {
                continue;
            }
            if !self.live.contains_key(&effect.entity) {
                continue;
            }
            let live = self
                .live
                .get_mut(&effect.entity)
                .context("live effect initialized")?;
            let offset = glam30::Vec3::from_array(effect.position);
            let camera = match camera {
                CameraSpec::LookAt {
                    position,
                    target,
                    vertical_fov,
                } => CameraSpec::LookAt {
                    position: *position - offset,
                    target: *target - offset,
                    vertical_fov: *vertical_fov,
                },
                CameraSpec::Orthographic { centre, world_size } => CameraSpec::Orthographic {
                    centre: *centre,
                    world_size: *world_size,
                },
                CameraSpec::FrameBounds {
                    min,
                    max,
                    direction,
                    padding,
                    vertical_fov,
                } => CameraSpec::FrameBounds {
                    min: *min - offset,
                    max: *max - offset,
                    direction: *direction,
                    padding: *padding,
                    vertical_fov: *vertical_fov,
                },
            };
            let view = View {
                camera,
                placement: Placement {
                    origin: glam30::Vec3::ZERO,
                    scale: effect.scale,
                },
                intensity: effect.intensity,
                exposure: frame.environment.exposure,
                scene_background: (!renderer.is_native()).then_some(color),
                scene_color: renderer.is_native().then_some(color),
                scene_depth: Some(depth),
                lighting: match frame.sun {
                    Some((direction, lux)) => Lighting {
                        sun_direction: glam30::Vec3::from_array(direction).normalize_or_zero(),
                        sun_color: glam30::Vec3::from_array(lux),
                        ..Lighting::default()
                    },
                    None => Lighting::default(),
                },
                ..View::default()
            };
            // Catch up only completed simulation ticks; redraws between ticks do
            // not age particles. Bound catch-up after a long paused capture.
            let count = if live.warm_remaining > 0 {
                // Match the RT view's initial library state. Subsequent frames
                // simulate only elapsed ticks and draw once, including stills.
                let count = live.warm_remaining.min(4);
                live.warm_remaining -= count;
                count.into()
            } else {
                frame.tick.saturating_sub(live.tick).min(8)
            };
            if renderer.is_native() {
                for _ in 1..count {
                    live.system.advance(renderer.gpu_mut(), &view)?;
                    self.gpu_ms += live
                        .system
                        .timings()
                        .iter()
                        .map(|pass| pass.milliseconds)
                        .sum::<f64>();
                }
                // The final tick and draw share frame constants, barriers and
                // one graph; earlier catch-up ticks stay simulation-only.
                live.system
                    .composite(renderer.gpu_mut(), &view, count > 0)?;
                self.gpu_ms += live
                    .system
                    .timings()
                    .iter()
                    .map(|pass| pass.milliseconds)
                    .sum::<f64>();
            } else if count == 0 && live.rendered {
                live.system.render(renderer.gpu_mut(), &view)?;
                self.gpu_ms += live
                    .system
                    .timings()
                    .iter()
                    .map(|pass| pass.milliseconds)
                    .sum::<f64>();
            } else {
                for _ in 0..count.max(1) {
                    live.system.step(renderer.gpu_mut(), &view)?;
                    self.gpu_ms += live
                        .system
                        .timings()
                        .iter()
                        .map(|pass| pass.milliseconds)
                        .sum::<f64>();
                }
            }
            live.tick = frame.tick;
            live.rendered = true;
            color = live.system.hdr_output();
        }
        Ok(color)
    }

    /// Release this view's particle resources before reusing its renderer/device.
    pub fn destroy(self, renderer: &mut Renderer) -> Result<()> {
        for (_, pending) in self.pending {
            pending.warmup.cancel(renderer.gpu_mut())?;
        }
        for (_, live) in self.live {
            live.system.retire(renderer.gpu_mut())?;
        }
        for (_, system) in self.systems {
            system.retire(renderer.gpu_mut())?;
        }
        Ok(())
    }

    /// Composites `frame`'s effects over the frame `renderer` just captured (call
    /// before resizing it) and replaces `capture`'s image. No effects: no change.
    pub fn composite(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        request: &CaptureRequest,
        capture: &mut Capture,
    ) -> Result<()> {
        if frame.effects.is_empty() {
            return Ok(());
        }
        let (image, sampled_index, state) = renderer.feature_color();
        let mut background = SceneColor {
            image,
            sampled_index,
            state,
        };
        let (image, sampled_index, state) = renderer.feature_depth();
        let depth = SceneDepth {
            image,
            sampled_index,
            state,
        };
        let lighting = || match frame.sun {
            Some((direction, lux)) => Lighting {
                sun_direction: glam30::Vec3::from_array(direction).normalize_or_zero(),
                sun_color: glam30::Vec3::from_array(lux),
                ..Lighting::default()
            },
            None => Lighting::default(),
        };
        let gpu = renderer.gpu_mut();
        for effect in &frame.effects {
            let key = (effect.effect.clone(), request.size);
            if !self.systems.contains_key(&key) {
                let definition = library(&effect.effect)?;
                let system = GpuSystem::new(
                    gpu,
                    &definition,
                    &cooked_root().join(&definition.name),
                    request.size,
                )?;
                self.systems.insert(key.clone(), system);
            }
            let system = self
                .systems
                .get_mut(&key)
                .context("effect system just inserted")?;
            let offset = glam30::Vec3::from_array(effect.position);
            let camera = match &request.camera {
                CameraSpec::LookAt {
                    position,
                    target,
                    vertical_fov,
                } => CameraSpec::LookAt {
                    position: *position - offset,
                    target: *target - offset,
                    vertical_fov: *vertical_fov,
                },
                other => *other,
            };
            let seed = u64::from(library(&effect.effect)?.seed);
            // The replay is always deterministic (authored seed, fixed tick), even
            // over a Low capture whose scene keeps live TAA.
            let replay = CaptureRequest {
                camera,
                seed,
                deterministic: true,
                tick: frame.tick.clamp(1, MAX_REPLAY),
                settle_frames: 1,
                marks: None,
                ..request.clone()
            };
            let composited = system.capture_over(
                gpu,
                &replay,
                View {
                    placement: Placement {
                        origin: glam30::Vec3::ZERO,
                        scale: effect.scale,
                    },
                    intensity: effect.intensity,
                    scene_background: Some(background),
                    scene_depth: Some(depth),
                    lighting: lighting(),
                    ..View::default()
                },
            )?;
            capture.image = composited.image;
            background = system.hdr_output();
        }
        // Grade the composited HDR with the renderer's look (exposure stays locked to
        // the request), exactly as presented frames are.
        let output =
            renderer.finish(background.image, background.sampled_index, background.state)?;
        crate::readback::replace(renderer, output, capture)?;
        Ok(())
    }
}
