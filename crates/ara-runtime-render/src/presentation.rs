//! One presentation path for windows, embedded surfaces and explicit readbacks.
use super::*;
use ara_gpu::{Access, Barrier, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk};
use ara_render::capture::{FeatureCapture, ShaderMetadata};
use ara_render_fidelity::presets::{Capabilities, FidelityPlan, Plan, RenderPath};
pub use ara_render_fidelity::presets::{QualityChoice, QualityPreset, RenderOptions};
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
mod raster;

impl Presenter {
    /// Reserve the initial GI volume and staging while a loading view remains
    /// visible. Call after a completed `present`, on the preparation worker, and
    /// continue presenting until true. Does not wait for the bulk probe bake.
    pub fn preallocate_startup_resources(&mut self) -> Result<bool> {
        self.drain()?;
        match (&mut self.raster, &mut self.renderer) {
            (Some(raster), Some(renderer)) => raster.preallocate_probe_resources(renderer),
            _ => Ok(true),
        }
    }
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
    /// Auto chooses a raster tier; experimental RT also requires RenderOptions.
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

    /// Take GPU pass timestamps (on by default). Off, Metal skips its timestamp
    /// dispatches and `stats().gpu_ms` and `pass_timings` stay empty. On the M5 Pro
    /// that measured neutral (Outpost) to 1.4% slower (Showcase), so players keep
    /// the default; it exists for overhead measurements.
    pub fn set_pass_timing(&mut self, enabled: bool) {
        self.pass_timing = enabled;
    }

    /// Enable/disable raster probe GI (enabled by default): content is acquired once
    /// in the background (ray-traced on a separate device, cached on disk by
    /// content) and relit every frame; frames show sky-only ambient until then.
    pub fn set_probe_gi(&mut self, enabled: bool) {
        self.probe_gi = enabled;
        if let Some(raster) = &mut self.raster {
            raster.set_probe_gi(enabled);
        }
    }

    /// Whether probe GI acquisition/delivery or settling remains pending. False
    /// when disabled, independently of emitter searches and model arrival fades.
    pub fn probe_gi_pending(&self) -> bool {
        self.raster
            .as_ref()
            .is_some_and(RasterView::probe_gi_pending)
    }

    /// Blocks until a pending probe GI acquisition is installed (captures and
    /// measurements that must not change lighting midway).
    pub fn wait_probe_gi(&mut self) -> Result<()> {
        // Installing the probes writes storage a frame in flight reads.
        self.drain()?;
        if let (Some(raster), Some(renderer)) = (&mut self.raster, &mut self.renderer) {
            raster.wait_probe_gi(renderer)?;
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
        let renderer = if let Some(gpu) = self.initial_device.take() {
            Renderer::with_device(scene, [64, 64], gpu)?
        } else {
            let create = if self.options.ray_tracing {
                Renderer::new
            } else {
                Renderer::new_raster
            };
            create(scene, [64, 64], &self.extensions, &self.cache)?
        };
        self.renderer = Some(renderer);
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
        self.stats = PresentStats::default();
        // Preparation writes host-visible storage the frame in flight still reads;
        // waiting it reports its GPU costs into this present's stats.
        self.drain()?;
        let started = Instant::now();
        self.ensure_device()?;
        let renderer = self
            .renderer
            .as_mut()
            .context("presentation device initialized")?;
        renderer.gpu_mut().set_pass_timing(self.pass_timing);
        renderer.gpu_mut().reset_timing_window();
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
        match plan.path {
            RenderPath::Raster(raster) => {
                if let Some(old) = self.live.take() {
                    old.destroy(renderer.gpu_mut())?;
                }
                self.prepare_raster(frame, raster, resolve)?;
            }
            RenderPath::Fidelity(settings) => {
                if let Some(raster) = self.raster.take() {
                    raster.destroy(renderer)?;
                    renderer.clear_native()?;
                }
                let change = match &mut self.live {
                    Some(live) => live.prepared.update_frame(frame)?,
                    None => FrameChange::Rebuild,
                };
                if matches!(change, FrameChange::Rebuild) {
                    let mut prepared = native::prepare(frame, resolve()?, texture_config())?;
                    prepared.enable_motion();
                    let bank = prepared.textures.fork_sources(texture_config())?;
                    let look = crate::environment(frame);
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
                let look = crate::environment(frame);
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

    /// Lets raster presents return once the frame is submitted, waiting it at the
    /// start of the next present or preparation (one frame of CPU/GPU overlap).
    /// Only for hosts whose later use of `output` is GPU work on the same queue
    /// (the window blit, the game UI composite); CPU readbacks `drain` first.
    /// Off by default; turning it off drains.
    pub fn set_frames_in_flight(&mut self, enabled: bool) -> Result<()> {
        self.frames_in_flight = enabled;
        if !enabled {
            self.drain()?;
        }
        Ok(())
    }

    /// Waits for the raster frame in flight, if any, and reports its GPU costs and
    /// device span in `stats` (with frames in flight they lag one present).
    pub fn drain(&mut self) -> Result<()> {
        let Some(submitted) = self.in_flight.take() else {
            return Ok(());
        };
        let started = Instant::now();
        let renderer = self
            .renderer
            .as_mut()
            .context("raster presentation device")?;
        let costs = RasterView::finish_batched(renderer, submitted)?;
        self.stats.in_flight_wait = started.elapsed();
        self.completed(costs);
        Ok(())
    }

    /// Records a completed raster frame's costs and the device's timing window.
    fn completed(&mut self, costs: GpuFrameCosts) {
        self.stats.gpu_ms = costs.accounted_ms();
        self.stats.gpu = Some(costs);
        self.timing_window();
    }

    /// The device span and submission gaps of the last completed work.
    fn timing_window(&mut self) {
        self.stats.gpu_span_ms = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.gpu().timing_window())
            .map(ara_gpu::vulkan::TimestampSpan::milliseconds);
        self.stats.gpu_submission_gap_ms = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.gpu().timing_window_gaps());
    }

    /// Presents through the selected renderer and publishes only a completed image;
    /// with frames in flight (`set_frames_in_flight`), a submitted one, whose stats
    /// arrive with the next present.
    pub fn present_with(
        &mut self,
        frame: &RenderFrame,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Presenter.present");
        let result = (|| {
            self.prepare_with(frame, resolve)?;
            let started = Instant::now();
            if let Some(raster) = &mut self.raster
                && self.live.is_none()
            {
                let renderer = self
                    .renderer
                    .as_mut()
                    .context("raster presentation device")?;
                // Publication joins the frame's completion scope: nothing waits here.
                let (published, size) = (&mut self.published, self.size);
                let submitted = raster.submit_batched(
                    renderer,
                    frame,
                    &camera(frame)?,
                    ViewMode::Lit,
                    |renderer| {
                        let source = renderer.output();
                        Published::publish(published, renderer.gpu_mut(), source, size).map(drop)
                    },
                )?;
                if self.frames_in_flight {
                    self.in_flight = Some(submitted);
                } else {
                    let costs = RasterView::finish_batched(renderer, submitted)?;
                    self.completed(costs);
                }
            } else {
                let source = if let Some(live) = &mut self.live {
                    let renderer = self.renderer.as_mut().context("presentation device")?;
                    let mut costs = live.render(renderer.gpu_mut(), frame)?;
                    costs.scene_update_ms = self.stats.gpu_ms;
                    self.stats.gpu_ms = costs.accounted_ms();
                    self.stats.gpu = Some(costs);
                    live.view.output()
                } else {
                    self.present_baseline(frame)?;
                    self.output.context("legacy completed output")?
                };
                self.publish(source)?;
                self.timing_window();
            }
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
                self.stats.gpu_span_ms = None;
                self.stats.gpu_submission_gap_ms = None;
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
        let _ = self.drain();
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
