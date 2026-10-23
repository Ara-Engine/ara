//! The scene's `VisualEffect`s composited into fidelity captures: each cooked library
//! effect (ara-vfx-gpu) is stepped to a steady state, drawn over the view's completed
//! HDR radiance and linear depth with the DDGI probe field as lighting (camera shifted
//! into the effect's local space), and the view's post stack then runs on the result.
//! This is the `feature_inputs`/`post_process` seam the fidelity view exposes; the
//! engine's own captures composite VFX over the raster renderer only.
use anyhow::{Context, Result};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::CameraSpec;
use ara_render_fidelity::{FidelityView, FrameInput, ImageInput};
use ara_runtime::extract::EffectView;
use ara_vfx_gpu::{GpuSystem, Lighting, ProbeLighting, SceneColor, SceneDepth, View};
use std::{collections::BTreeMap, path::PathBuf};

/// Ticks an effect runs before a still (fires reach steady state well before).
const WARM_TICKS: u32 = 150;

fn cooked_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ara-vfx-gpu/cooked"
    ))
}

/// Effect systems for one view size.
pub struct Effects {
    effects: Vec<EffectView>,
    systems: BTreeMap<String, GpuSystem>,
    size: [u32; 2],
    sun: (glam::Vec3, glam::Vec3),
}

impl Effects {
    /// Systems for `effects` at the view's transport size.
    pub fn new(
        gpu: &mut VulkanDevice,
        effects: &[EffectView],
        size: [u32; 2],
        sun: (glam::Vec3, glam::Vec3),
    ) -> Result<Self> {
        let mut systems = BTreeMap::new();
        for effect in effects {
            if systems.contains_key(&effect.effect) {
                continue;
            }
            let definition = ara_vfx::samples::all()
                .into_iter()
                .map(|(e, _)| e)
                .find(|e| e.name == effect.effect)
                .with_context(|| format!("no cooked VFX effect `{}`", effect.effect))?;
            let system = GpuSystem::new(
                gpu,
                &definition,
                &cooked_root().join(&definition.name),
                size,
            )?;
            systems.insert(effect.effect.clone(), system);
        }
        Ok(Self {
            effects: effects.to_vec(),
            systems,
            size,
            sun,
        })
    }

    /// Whether there is anything to composite.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Composites every effect over the view's last frame and runs post on it.
    pub fn composite(
        &mut self,
        gpu: &mut VulkanDevice,
        view: &mut FidelityView,
        camera: &CameraSpec,
        tick: u32,
    ) -> Result<()> {
        let inputs = view.feature_inputs()?;
        let mut background = SceneColor {
            image: inputs.radiance.image,
            sampled_index: inputs.radiance.sampled_index,
            state: inputs.radiance.state,
        };
        let depth = || SceneDepth {
            image: inputs.depth.image,
            sampled_index: inputs.depth.sampled_index,
            state: inputs.depth.state,
        };
        let lighting = || Lighting {
            ambient: glam::Vec3::new(0.1, 0.1, 0.12),
            sun_direction: self.sun.0,
            sun_color: self.sun.1,
            probes: Some(ProbeLighting {
                buffer: inputs.probes,
                index: inputs.probe_index,
                origin: inputs.volume.origin,
                spacing: inputs.volume.spacing,
                counts: inputs.volume.counts,
                state: inputs.probe_state,
            }),
        };
        for effect in &self.effects {
            let system = self
                .systems
                .get_mut(&effect.effect)
                .context("effect system")?;
            let offset = glam::Vec3::from_array(effect.position);
            let local = match camera {
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
            let frame = View {
                camera: local,
                scene_background: Some(background),
                scene_depth: Some(depth()),
                linear_depth: true,
                lighting: lighting(),
                exposure: 1.0,
                ..View::default()
            };
            for _ in 0..WARM_TICKS {
                system.step(gpu, &frame)?;
            }
            system.render(gpu, &frame)?;
            background = system.hdr_output();
        }
        let _ = self.size;
        view.post_process(
            gpu,
            &FrameInput {
                camera,
                tick,
                delta_seconds: 1.0 / 60.0,
                reset: false,
                lighting_changed: false,
                deterministic: false,
            },
            ImageInput {
                image: background.image,
                sampled_index: background.sampled_index,
                state: background.state,
            },
        )
    }

    /// Releases the systems before the device.
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for (_, system) in self.systems {
            system.retire(gpu)?;
        }
        Ok(())
    }
}
