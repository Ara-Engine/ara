//! The scene's `VisualEffect`s composited into fidelity views: each cooked library
//! effect (ara-vfx-gpu) is stepped to a steady state, drawn over the view's completed
//! HDR radiance and linear depth with the DDGI probe field as lighting (camera shifted
//! into the effect's local space), and the view's post stack then runs on the result.
//! Uses the view's `feature_inputs`/`post_process` seam so particles share its
//! exposure, tone mapper and temporal reconstruction.
use anyhow::{Context, Result};
use ara_gpu::GpuDevice;
use ara_render::scene::CameraSpec;
use ara_render_fidelity::{FidelityView, FrameInput, ImageInput};
use ara_runtime::extract::EffectView;
use ara_vfx_gpu::{GpuSystem, Lighting, Placement, ProbeLighting, SceneColor, SceneDepth, View};
use std::{collections::BTreeMap, path::PathBuf};

/// Ticks an effect runs before its first composite (fires reach steady state well
/// before); later composites advance by the elapsed ticks they are given, so a run of
/// composited frames is a coherent animation the temporal antialiasing converges on.
const WARM_TICKS: u32 = 150;

fn cooked_root() -> PathBuf {
    ara_gpu::cooked::resource_dir(
        "ara-vfx-gpu",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../ara-vfx-gpu/cooked"),
    )
}

/// Effect systems for one view size, one per effect entity.
pub struct Effects {
    effects: Vec<EffectView>,
    /// Entity → (effect name, system, warmed).
    systems: BTreeMap<u64, (String, GpuSystem, bool)>,
    size: [u32; 2],
    sun: (glam30::Vec3, glam30::Vec3),
    gpu_ms: f64,
}

impl Effects {
    /// Systems for `effects` at the view's transport size. `sun` is the direction
    /// toward the sun and the illuminance reaching the effects (after atmosphere).
    pub fn new(
        gpu: &mut GpuDevice,
        effects: &[EffectView],
        size: [u32; 2],
        sun: (glam30::Vec3, glam30::Vec3),
    ) -> Result<Self> {
        let mut result = Self {
            effects: Vec::new(),
            systems: BTreeMap::new(),
            size,
            sun,
            gpu_ms: 0.,
        };
        result.update(gpu, effects, sun)?;
        Ok(result)
    }

    /// Frame-boundary update for live views: the extracted effect list (poses,
    /// scale, intensity) and sun. New entities get systems (warmed on their first
    /// composite); removed entities retire theirs, as does an entity whose effect changed.
    pub fn update(
        &mut self,
        gpu: &mut GpuDevice,
        effects: &[EffectView],
        sun: (glam30::Vec3, glam30::Vec3),
    ) -> Result<()> {
        if self.effects.len() == effects.len()
            && self
                .effects
                .iter()
                .zip(effects)
                .all(|(old, new)| old.entity == new.entity && old.effect == new.effect)
        {
            for (old, new) in self.effects.iter_mut().zip(effects) {
                old.position = new.position;
                old.scale = new.scale;
                old.intensity = new.intensity;
            }
            self.sun = sun;
            return Ok(());
        }
        let stale: Vec<u64> = self
            .systems
            .iter()
            .filter(|(entity, (name, _, _))| {
                !effects
                    .iter()
                    .any(|e| e.entity == **entity && e.effect == *name)
            })
            .map(|(entity, _)| *entity)
            .collect();
        for entity in stale {
            if let Some((_, system, _)) = self.systems.remove(&entity) {
                system.retire(gpu)?;
            }
        }
        for effect in effects {
            if self.systems.contains_key(&effect.entity) {
                continue;
            }
            let definition = ara_vfx::samples::find(&effect.effect)
                .with_context(|| format!("no cooked VFX effect `{}`", effect.effect))?;
            let system = GpuSystem::new(
                gpu,
                &definition,
                &cooked_root().join(&definition.name),
                self.size,
            )?;
            self.systems
                .insert(effect.entity, (effect.effect.clone(), system, false));
        }
        self.effects = effects.to_vec();
        self.sun = sun;
        Ok(())
    }

    /// Whether there is anything to composite.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// GPU time of all particle steps and draws in the last composite. The view's
    /// subsequent post passes are accounted for by `FidelityView::timings`.
    pub fn gpu_ms(&self) -> f64 {
        self.gpu_ms
    }

    /// Changes working targets at a view boundary. Definitions and placements stay
    /// intact; particle systems warm again at the new transport resolution.
    pub fn resize(&mut self, gpu: &mut GpuDevice, size: [u32; 2]) -> Result<()> {
        if self.size != size {
            for (_, (_, system, _)) in std::mem::take(&mut self.systems) {
                system.retire(gpu)?;
            }
            let effects = std::mem::take(&mut self.effects);
            self.size = size;
            self.update(gpu, &effects, self.sun)?;
        }
        Ok(())
    }

    /// Restarts explicit captures from the same particle simulation state. Live
    /// presentation keeps its systems; only a reproducible readback calls this.
    pub fn reset(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        for (_, (_, system, _)) in std::mem::take(&mut self.systems) {
            system.retire(gpu)?;
        }
        self.effects.clear();
        Ok(())
    }

    /// Composites every effect over the view's last frame, advancing each by one
    /// simulation tick (captures), and runs post on it.
    pub fn composite(
        &mut self,
        gpu: &mut GpuDevice,
        view: &mut FidelityView,
        camera: &CameraSpec,
        tick: u32,
    ) -> Result<()> {
        self.composite_ticks(gpu, view, camera, tick, 1)
    }

    /// Composites every effect over the view's last frame after advancing each by
    /// `ticks` completed simulation ticks (0 redraws the current particles; a new
    /// system first warms to steady state), then runs the view's post on the result.
    /// Call once per presented frame for live views.
    pub fn composite_ticks(
        &mut self,
        gpu: &mut GpuDevice,
        view: &mut FidelityView,
        camera: &CameraSpec,
        tick: u32,
        ticks: u32,
    ) -> Result<()> {
        self.gpu_ms = 0.;
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
            ambient: glam30::Vec3::new(0.1, 0.1, 0.12),
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
            let (_, system, warmed) = self
                .systems
                .get_mut(&effect.entity)
                .context("effect system")?;
            let frame = View {
                camera: *camera,
                placement: Placement {
                    origin: glam30::Vec3::from_array(effect.position),
                    scale: effect.scale,
                },
                intensity: effect.intensity,
                scene_background: Some(background),
                scene_depth: Some(depth()),
                linear_depth: true,
                lighting: lighting(),
                exposure: 1.0,
                ..View::default()
            };
            let steps = if *warmed { ticks } else { WARM_TICKS };
            *warmed = true;
            // Simulate the elapsed ticks; only the presented frame is drawn.
            for _ in 0..steps {
                system.advance(gpu, &frame)?;
                self.gpu_ms += system
                    .timings()
                    .iter()
                    .map(|pass| pass.milliseconds)
                    .sum::<f64>();
            }
            system.render(gpu, &frame)?;
            self.gpu_ms += system
                .timings()
                .iter()
                .map(|pass| pass.milliseconds)
                .sum::<f64>();
            background = system.hdr_output();
        }
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
    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for (_, (_, system, _)) in self.systems {
            system.retire(gpu)?;
        }
        Ok(())
    }
}
