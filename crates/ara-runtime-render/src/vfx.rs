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
use ara_vfx_gpu::{GpuSystem, Lighting, SceneColor, SceneDepth, View};
use std::{collections::BTreeMap, path::PathBuf};

/// Ticks replayed at most per capture (effects loop well within it).
const MAX_REPLAY: u64 = 240;

/// The cooked library effects shipped with ara-vfx-gpu (`cooked/<name>`); a project
/// asset pipeline for authored effects is the next step.
fn cooked_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ara-vfx-gpu/cooked"
    ))
}

fn library(name: &str) -> Result<Effect> {
    ara_vfx::samples::all()
        .into_iter()
        .map(|(effect, _)| effect)
        .find(|effect| effect.name == name)
        .with_context(|| format!("no cooked VFX effect `{name}`"))
}

/// Effect systems on the renderer's device, by effect name and capture size.
#[derive(Default)]
pub struct Effects {
    systems: BTreeMap<(String, [u32; 2]), GpuSystem>,
}

impl Effects {
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
                other => other.clone(),
            };
            let seed = u64::from(library(&effect.effect)?.seed);
            let replay = CaptureRequest {
                camera,
                seed,
                tick: frame.tick.clamp(1, MAX_REPLAY),
                settle_frames: 1,
                marks: None,
                ..request.clone()
            };
            let composited = system.capture_over(
                gpu,
                &replay,
                View {
                    scene_background: Some(background),
                    scene_depth: Some(depth),
                    lighting: lighting(),
                    ..View::default()
                },
            )?;
            capture.image = composited.image;
            background = system.hdr_output();
        }
        Ok(())
    }
}
