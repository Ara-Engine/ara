//! One presentation path for windows, embedded surfaces and explicit readbacks.
use super::*;
use ara_gpu::{
    Access, Barrier, Device, Image, Queue, Resource, commands::Command, vk, vulkan::VulkanDevice,
};
use ara_render::capture::{FeatureCapture, ShaderMetadata};
use ara_render_fidelity::presets::{Capabilities, FidelityPlan, Plan, RenderOptions, RenderPath};
pub use ara_render_fidelity::presets::{QualityChoice, QualityPreset};
use ara_render_fidelity::{FidelityView, FrameInput, PostSettings, environment::FrameEnvironment};
use fidelity::native::{self, Content, FrameChange, NativeConfig, Prepared};

fn texture_config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1024 * 1024 * 1024,
        upload_bytes: 16 * 1024 * 1024,
    }
}

pub(super) struct Live {
    prepared: Prepared,
    view: FidelityView,
    effects: fidelity::vfx::Effects,
    plan: FidelityPlan,
    look: FrameEnvironment,
    tick: Option<u64>,
    temporal: u32,
    lighting_changed: bool,
}

/// A completed display image independent of view targets and scene generations.
pub struct PresentationImage {
    pub(super) image: Image,
    size: [u32; 2],
}
pub(super) type Published = PresentationImage;

mod capture;
mod live;
mod published;

impl Presenter {
    /// Timestamp results from the active view, excluding readback and display transfers.
    pub fn pass_timings(&self) -> &[ara_gpu::vulkan::PassTiming] {
        if let Some(live) = &self.live {
            live.view.timings()
        } else {
            self.renderer
                .as_ref()
                .map_or(&[], |renderer| renderer.timings())
        }
    }

    /// Development shader provenance for a completed frame.
    pub fn shader_status(&self) -> ara_gpu::vulkan::ShaderStatus {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.gpu().shader_status())
            .unwrap_or_default()
    }

    /// Queue a shader replacement without blocking this frame on the driver compiler.
    pub fn reload_shaders(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            renderer.gpu_mut().request_shader_reload();
        }
    }

    /// Selects a user override, or the project's setting when no override exists.
    /// Auto chooses the hardware preset; unsupported RT presets explicitly resolve to Low.
    pub fn set_quality(&mut self, quality: QualityChoice) {
        self.choice = quality;
    }

    /// Replaces the frame's authored look on the raster path (look development and
    /// scoring at a fixed exposure); `None` returns to the authored look.
    pub fn set_look_override(&mut self, look: Option<FrameEnvironment>) {
        self.look_override = look;
    }

    /// Renderer selection: raster everywhere, or the opt-in ray-traced tiers.
    pub fn set_render_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

    /// Opt in to probe GI on the raster path: each content change is acquired once
    /// in the background (ray-traced on a separate device, cached on disk by
    /// content) and relit every frame; frames show sky-only ambient until then.
    pub fn set_probe_gi(&mut self, enabled: bool) {
        self.probe_gi = enabled.then(|| crate::gi::Acquisition::new(self.cache.clone()));
    }

    /// Whether probe GI acquisition is still running or queued: a view that stops
    /// redrawing once converged must keep presenting until the result arrives.
    pub fn probe_gi_pending(&self) -> bool {
        self.probe_gi.as_ref().is_some_and(|gi| gi.is_pending())
    }

    /// Blocks until a pending probe GI acquisition is installed (captures and
    /// measurements that must not change lighting midway).
    pub fn wait_probe_gi(&mut self) -> Result<()> {
        let surfaces = self.probe_gi.as_mut().and_then(|gi| gi.poll(true));
        if let (Some(surfaces), Some(renderer)) = (surfaces, self.renderer.as_mut()) {
            renderer.set_probe_surfaces(Some(&surfaces))?;
        }
        Ok(())
    }

    /// Preset of the last successfully published frame, or the selected warm-up plan.
    pub fn quality(&self) -> Option<QualityPreset> {
        self.published_quality.or(self.plan.map(|plan| plan.preset))
    }

    /// A failed replacement's diagnostic. The last completed display remains valid.
    pub fn presentation_error(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Requested display size, independent of the bootstrap raster device's targets.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Extent of `output()`, including a last-good frame retained across a failed resize.
    pub fn output_size(&self) -> [u32; 2] {
        self.published
            .as_ref()
            .map_or(self.size, |image| image.size)
    }

    fn ensure_device(&mut self) -> Result<()> {
        if self.renderer.is_some() {
            return Ok(());
        }
        let mut warm = RenderFrame::default();
        warm.draws.push(Draw {
            entity: 0,
            address: "renderer-bootstrap".into(),
            label: "Renderer bootstrap".into(),
            geometry: Geometry::Shape {
                shape: ara_runtime::components::MeshShape::Box,
                size: [1.; 3],
            },
            world: glam::Mat4::IDENTITY,
            color: [1.; 4],
            roughness: 0.5,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
        });
        let scene = self.scene(&warm, &[])?;
        self.renderer = Some(Renderer::new(
            scene,
            [64, 64],
            &self.extensions,
            &self.cache,
        )?);
        Ok(())
    }

    /// Prepares an extraction, resolving terrain/prefab content only on structural changes.
    /// The resolver may read published assets; it is never invoked for ordinary motion.
    pub fn prepare_with(
        &mut self,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Presenter.prepare");
        let started = Instant::now();
        self.ensure_device()?;
        let renderer = self
            .renderer
            .as_mut()
            .context("presentation device initialized")?;
        let plan = Plan::resolve(
            self.choice,
            &Capabilities::of(renderer.gpu()),
            self.size,
            self.options,
        )?;
        if self.plan != Some(plan) {
            eprintln!(
                "Ara presentation: {} (requested {}, {:?})",
                plan.preset, plan.requested, plan.path
            );
        }
        self.stats = PresentStats::default();
        match plan.path {
            RenderPath::Raster(raster) => {
                if let Some(old) = self.live.take() {
                    old.destroy(renderer.gpu_mut())?;
                }
                if renderer.size() != self.size {
                    renderer.resize(self.size)?;
                }
                if *renderer.quality() != raster.quality {
                    renderer.set_quality(raster.quality)?;
                }
                let content = self.prepare_baseline(frame, resolve)?;
                let renderer = self.renderer.as_mut().context("baseline renderer")?;
                if let Some(gi) = &mut self.probe_gi {
                    // New content: drop stale probes and acquire in the background.
                    if let Some(content) = content {
                        let key = self.layout.iter().fold(FNV, |h, (a, b, c)| {
                            [a, b, c].iter().fold(h, |h, v| fnv(h, &v.to_le_bytes()))
                        });
                        let key = frame
                            .draws
                            .iter()
                            .filter(|d| d.skin.is_none())
                            .fold(key, |h, d| {
                                fnv(h, bytemuck::bytes_of(&d.world.to_cols_array()))
                            });
                        renderer.set_probe_surfaces(None)?;
                        gi.request(key, frame, || Ok(content))?;
                    }
                    if let Some(surfaces) = gi.poll(false) {
                        renderer.set_probe_surfaces(Some(&surfaces))?;
                    }
                }
            }
            RenderPath::Fidelity(settings) => {
                let change = match &mut self.live {
                    Some(live) => live.prepared.update_frame(frame)?,
                    None => FrameChange::Rebuild,
                };
                if matches!(change, FrameChange::Rebuild) {
                    let mut prepared = native::prepare(frame, resolve()?, texture_config())?;
                    prepared.enable_motion();
                    let bank = prepared.textures.fork_sources(texture_config())?;
                    let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
                    match &mut self.live {
                        Some(live) => {
                            live.view.replace_native_scene(
                                renderer.gpu_mut(),
                                &prepared.scene,
                                bank,
                            )?;
                            live.prepared = prepared;
                            live.tick = None;
                        }
                        None => {
                            let view = FidelityView::new_native(
                                renderer.gpu_mut(),
                                &prepared.scene,
                                bank,
                                settings.settings(look.post(PostSettings::default())),
                                None,
                            )?;
                            let effects = fidelity::vfx::Effects::new(
                                renderer.gpu_mut(),
                                &[],
                                settings.render_size,
                                (
                                    prepared.scene.sun_direction,
                                    look.ground_sun(
                                        prepared.scene.sun_direction,
                                        prepared.scene.sun_lux,
                                    ),
                                ),
                            )?;
                            self.live = Some(Live {
                                prepared,
                                view,
                                effects,
                                plan: settings,
                                look,
                                tick: None,
                                temporal: 0,
                                lighting_changed: true,
                            });
                        }
                    }
                    self.stats.rebuilt = true;
                } else if let FrameChange::Ready { poses, skins } = change {
                    self.stats.updated = poses + skins;
                }
                let live = self.live.as_mut().context("native view initialized")?;
                if live.plan != settings {
                    let old = std::mem::replace(
                        &mut live.effects,
                        fidelity::vfx::Effects::new(
                            renderer.gpu_mut(),
                            &[],
                            settings.render_size,
                            (
                                live.prepared.scene.sun_direction,
                                live.prepared.scene.sun_lux,
                            ),
                        )?,
                    );
                    old.destroy(renderer.gpu_mut())?;
                    live.view.resize_unpublished(
                        renderer.gpu_mut(),
                        settings.render_size,
                        settings.display_size,
                    )?;
                    live.plan = settings;
                }
                let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
                if live.look != look || live.tick.is_none() {
                    live.view.set_environment(&look)?;
                    live.look = look;
                }
                settings.configure(renderer.gpu_mut(), &mut live.view)?;
                live.view.prepare_transport(renderer.gpu_mut())?;
                let update = live
                    .view
                    .update_native_scene(renderer.gpu_mut(), &live.prepared.scene)?;
                live.lighting_changed |= update.lighting;
                self.stats.gpu_ms = update.gpu_ms;
                self.stats.gpu = Some(GpuFrameCosts {
                    scene_update_ms: update.gpu_ms,
                    ..Default::default()
                });
            }
        }
        self.plan = Some(plan);
        self.stats.prepare = started.elapsed();
        Ok(())
    }

    /// Default hardware-selected preparation; callers with terrain use `prepare_with`.
    pub fn prepare(&mut self, frame: &RenderFrame) -> Result<()> {
        self.prepare_with(frame, || Ok(Content::default()))
    }

    /// Presents through the selected renderer and publishes only a completed image.
    pub fn present_with(
        &mut self,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Presenter.present");
        let result = (|| {
            self.prepare_with(frame, resolve)?;
            let started = Instant::now();
            let source = if let Some(live) = &mut self.live {
                let renderer = self.renderer.as_mut().context("presentation device")?;
                let mut costs = live.render(renderer.gpu_mut(), frame)?;
                costs.scene_update_ms = self.stats.gpu_ms;
                self.stats.gpu_ms = costs.accounted_ms();
                self.stats.gpu = Some(costs);
                live.view.output()
            } else {
                self.present_baseline(frame)?;
                self.output.context("baseline completed output")?
            };
            self.publish(source)?;
            self.stats.render = started.elapsed();
            self.published_quality = self.plan.map(|plan| plan.preset);
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.failure = None;
                Ok(())
            }
            Err(error) => {
                if self.failure.is_none() {
                    eprintln!(
                        "ARA_GPU_BACKEND: presentation retains last completed frame: {error:#}"
                    );
                    self.failure = Some(format!("{error:#}"));
                }
                if self.published.is_some() {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Presents model-only content; resolved terrain/prefabs use `present_with`.
    pub fn present(&mut self, frame: &RenderFrame) -> Result<()> {
        self.present_with(frame, || Ok(Content::default()))
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            if let Some(live) = self.live.take() {
                let _ = live.destroy(renderer.gpu_mut());
            }
            if let Some(image) = self.published.take() {
                let _ = renderer.gpu_mut().retire(Resource::Image(image.image));
            }
            let _ = renderer.gpu_mut().collect();
        }
    }
}
