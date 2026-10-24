//! Native raster view shared by the editor, Play and installed players.
use crate::{
    GpuFrameCosts, Renderer,
    fidelity::native::{self, Content, FrameChange, NativeConfig, Prepared},
    vfx,
};
use anyhow::{Context, Result, ensure};
use ara_render::capture::{Capture, CaptureRequest, ViewMode};
use ara_render::look::gi::ProbeUpdate;
use ara_render::scene::CameraSpec;
use ara_render_fidelity::{environment::FrameEnvironment, presets::RasterPlan};
use ara_runtime::extract::RenderFrame;
use std::path::PathBuf;

/// Content-boundary result; ordinary movement keeps native allocations resident.
#[derive(Debug, Default, Clone, Copy)]
pub struct RasterUpdate {
    /// A complete geometry/material/texture generation was adopted.
    pub rebuilt: bool,
    /// Root transforms and skin geometries updated in place.
    pub updated: usize,
}

/// Derived native content and VFX on a caller-owned renderer/device. It never
/// discovers, cooks or imports assets; the resolver supplies published generations.
pub struct RasterView {
    prepared: Option<Prepared>,
    effects: vfx::Effects,
    gi: Option<crate::gi::Acquisition>,
    /// Static/dynamic split of the instances for probe GI.
    mobility: crate::gi::Mobility,
    /// Delivered probe updates not applied yet (large ones go a slice per frame).
    gi_updates: std::collections::VecDeque<ProbeUpdate>,
    /// Emissive surfaces of the current generation and their lights, uploaded
    /// after the scene's own lights.
    emitters: Option<crate::emitters::Emitters>,
    /// The emitter search for the current generation, running off this thread.
    emitter_job: Option<std::thread::JoinHandle<crate::emitters::Emitters>>,
    emitter_lights: Vec<ara_render::scene::Light>,
    cache: PathBuf,
    key: u64,
    pending_gi: bool,
    look: Option<FrameEnvironment>,
}

impl RasterView {
    /// Creates CPU view state. Probe surfaces are acquired asynchronously and
    /// disk-cached; no GPU work or file IO happens in this constructor.
    pub fn new(cache: impl Into<PathBuf>) -> Self {
        let cache = cache.into();
        Self {
            prepared: None,
            effects: Default::default(),
            gi: Some(crate::gi::Acquisition::new(cache.clone())),
            mobility: Default::default(),
            gi_updates: Default::default(),
            emitters: None,
            emitter_job: None,
            emitter_lights: Vec::new(),
            cache,
            key: crate::FNV,
            pending_gi: false,
            look: None,
        }
    }

    /// Enable/disable probe acquisition. Preset budgets govern relighting; the
    /// renderer keeps sky ambient until the asynchronous surface cache arrives.
    pub fn set_probe_gi(&mut self, enabled: bool) {
        if enabled == self.gi.is_some() {
            return;
        }
        self.gi = enabled.then(|| crate::gi::Acquisition::new(self.cache.clone()));
        self.pending_gi = enabled;
    }

    /// Keep progressive editor views polling until acquired probe surfaces arrive
    /// and moved instances settle back into them.
    pub fn needs_frame(&self) -> bool {
        self.emitter_job.is_some()
            || self.gi.as_ref().is_some_and(|gi| {
                gi.is_pending() || self.mobility.settling() || !self.gi_updates.is_empty()
            })
    }

    /// Installs the finished (or, blocking, the running) emitter search.
    fn adopt_emitters(&mut self) {
        if let Some(job) = self.emitter_job.take() {
            match job.join() {
                Ok(emitters) => self.emitters = (!emitters.is_empty()).then_some(emitters),
                Err(_) => eprintln!("ara-runtime-render: the emitter search panicked"),
            }
        }
    }

    /// Change derived capture/probe labels at an editor control boundary without
    /// replacing geometry, residency or temporal state.
    pub fn rename(&mut self, entity: u64, name: &str) -> Result<()> {
        let instance = self
            .prepared
            .as_mut()
            .context("native raster content")?
            .scene
            .instances
            .iter_mut()
            .find(|instance| instance.entity == entity)
            .context("native raster label entity missing")?;
        instance.name.clear();
        instance.name.push_str(name);
        Ok(())
    }

    /// Resolve a new content generation once, or update transforms/material factors
    /// and skin poses. Rejected generations leave the existing renderer content live.
    pub fn prepare(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        plan: RasterPlan,
        look: FrameEnvironment,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<RasterUpdate> {
        self.prepare_inner(renderer, frame, plan, look, resolve, true)
    }

    /// Render an unchanged retained extraction without revisiting its draws.
    /// Camera/quality changes and asynchronous probe delivery still progress. The
    /// caller must use `prepare` after any pose, factor, light or content edit.
    pub fn prepare_unchanged(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        plan: RasterPlan,
        look: FrameEnvironment,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<RasterUpdate> {
        self.prepare_inner(renderer, frame, plan, look, resolve, false)
    }

    fn prepare_inner(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        plan: RasterPlan,
        look: FrameEnvironment,
        resolve: impl FnOnce() -> Result<Content>,
        snapshot_changed: bool,
    ) -> Result<RasterUpdate> {
        ara_gpu::cpu_scope!("RasterView.prepare");
        let result = (|| {
            let change = if snapshot_changed || self.prepared.is_none() {
                self.prepared
                    .as_mut()
                    .map(|p| p.update_frame(frame))
                    .transpose()?
                    .unwrap_or(FrameChange::Rebuild)
            } else {
                FrameChange::Ready { poses: 0, skins: 0 }
            };
            let mut resolver = Some(resolve);
            let mut bake_content = None;
            let mut update = RasterUpdate::default();
            if matches!(change, FrameChange::Rebuild) {
                let content = resolver.take().context("raster asset resolver")?()?;
                if self.gi.is_some() && plan.quality.gi_budget > 0 {
                    // Publication boundary only: the worker owns immutable source
                    // assets, while the presented scene continues accepting motion.
                    bake_content = Some(content.clone());
                }
                let config = texture_config();
                let mut prepared = native::prepare(frame, content, config)?;
                prepared.enable_motion();
                let textures = prepared.textures.fork_sources(config)?;
                renderer.replace_native_scene(&prepared.scene, Box::new(textures))?;
                self.key = content_key(&prepared);
                // Emitter lights arrive from a worker; until then emitters light
                // nothing (their emission is out of the probes too).
                let job = crate::emitters::Job::new(
                    &prepared.scene,
                    &crate::gi::shading(&prepared),
                    |texture, side| prepared.textures.texels(texture, side),
                );
                self.emitters = None;
                self.emitter_job = if job.is_empty() {
                    None
                } else {
                    Some(
                        std::thread::Builder::new()
                            .name("ara-emitters".into())
                            .spawn(move || job.run())
                            .context("emitter search thread")?,
                    )
                };
                self.prepared = Some(prepared);
                self.pending_gi = true;
                update.rebuilt = true;
            } else if let FrameChange::Ready { poses, skins } = change {
                update.updated = poses + skins;
            }
            if renderer.size() != plan.render_size {
                renderer.resize(plan.render_size)?;
            }
            if *renderer.quality() != plan.quality {
                renderer.set_quality(plan.quality)?;
            }
            let raster_look = look.raster();
            if *renderer.look() != raster_look {
                renderer.set_look(raster_look)?;
            }
            if self
                .emitter_job
                .as_ref()
                .is_some_and(|job| job.is_finished())
            {
                self.adopt_emitters();
            }
            let prepared = self.prepared.as_mut().context("native raster content")?;
            // Emitter lights follow their instances and ride after the scene's
            // lights, within the shared light budget.
            match &mut self.emitters {
                Some(emitters) => {
                    let budget = (plan.quality.emitter_lights as usize).min(
                        ara_render::scene::MAX_LOCAL_LIGHTS
                            .saturating_sub(prepared.scene.lights.len()),
                    );
                    if let Some(lights) = emitters.update(&prepared.scene, budget) {
                        self.emitter_lights = lights;
                        if update.rebuilt {
                            eprintln!(
                                "ara-runtime-render: emissive surfaces light the scene as {} lights",
                                self.emitter_lights.len()
                            );
                        }
                    }
                }
                None => self.emitter_lights.clear(),
            }
            let own = prepared.scene.lights.len();
            prepared
                .scene
                .lights
                .extend_from_slice(&self.emitter_lights);
            let unchanged = ara_render::instanced::Changes::default();
            let updated = renderer.update_native_changed(
                &prepared.scene,
                if snapshot_changed {
                    prepared.changes()
                } else {
                    &unchanged
                },
            );
            prepared.scene.lights.truncate(own);
            updated?;
            self.look = Some(look);
            if plan.quality.gi_budget > 0
                && let Some(gi) = &mut self.gi
            {
                // Moved instances leave the probe surfaces until they rest again; the
                // stale surfaces stay in use until the re-traced ones land.
                let scene = &self
                    .prepared
                    .as_ref()
                    .context("native raster content")?
                    .scene;
                let edited = self
                    .mobility
                    .observe(scene, update.rebuilt || update.updated > 0);
                if self.pending_gi {
                    let content = match bake_content {
                        Some(content) => content,
                        None => resolver.take().context("probe asset resolver")?()?,
                    };
                    gi.request(self.key, frame, || Ok(content), self.mobility.dynamic())?;
                    self.pending_gi = false;
                } else if edited {
                    gi.edit(self.mobility.worlds(), self.mobility.dynamic())?;
                }
                while let Some(delivery) = gi.poll(false) {
                    queue_probe_gi(&mut self.gi_updates, delivery);
                }
                apply_probe_gi(renderer, &mut self.gi_updates, PROBES_PER_FRAME)?;
            }
            Ok(update)
        })();
        if result.is_err() {
            // Extraction may already have adopted a pose before quality/upload
            // failed. Rebuild CPU content on retry so no dirty edit is forgotten;
            // the renderer and its published last-good image remain owned/live.
            self.prepared = None;
        }
        result
    }

    /// Present native terrain/foliage through the G-buffer, then particles and one
    /// shared temporal/post chain. Debug modes use the same geometry and material data.
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
    ) -> Result<GpuFrameCosts> {
        ara_gpu::cpu_scope!("RasterView.render");
        ensure!(
            renderer.is_native(),
            "raster content must be prepared before rendering"
        );
        let look = self.look.context("raster look must be prepared")?;
        let lit = matches!(mode, ViewMode::Lit);
        renderer.set_deferred_post(lit && !frame.effects.is_empty());
        renderer.render_view(
            camera,
            look.manual_exposure(frame.environment.exposure),
            mode,
        )?;
        if lit {
            let (image, sampled_index, state) = renderer.feature_color();
            let color = ara_vfx_gpu::SceneColor {
                image,
                sampled_index,
                state,
            };
            let (image, sampled_index, state) = renderer.feature_depth();
            let depth = ara_vfx_gpu::SceneDepth {
                image,
                sampled_index,
                state,
            };
            let color = self
                .effects
                .present(renderer, frame, camera, color, depth)?;
            if !frame.effects.is_empty() {
                renderer.finish(color.image, color.sampled_index, color.state)?;
            }
        }
        let textures = renderer
            .native_texture_stats()
            .context("native residency statistics")?;
        Ok(GpuFrameCosts {
            scene_update_ms: renderer.native_update_gpu_ms().unwrap_or(0.),
            texture_upload_ms: textures.upload_gpu_ms,
            texture_uploaded_bytes: textures.uploaded_bytes,
            texture_pending: textures.pending_textures,
            texture_resident_bytes: textures.resident_bytes,
            raster_ms: renderer
                .timings()
                .iter()
                .map(|pass| pass.milliseconds)
                .sum(),
            effects_ms: if lit { self.effects.gpu_ms() } else { 0. },
            ..Default::default()
        })
    }

    /// Wait only at an explicit capture/measurement boundary, never in live rendering.
    pub fn wait_probe_gi(&mut self, renderer: &mut Renderer) -> Result<()> {
        self.adopt_emitters();
        if let Some(gi) = &mut self.gi {
            while let Some(update) = gi.poll(true) {
                queue_probe_gi(&mut self.gi_updates, update);
            }
        }
        apply_probe_gi(renderer, &mut self.gi_updates, usize::MAX)?;
        Ok(())
    }

    /// Capture this presentation, retaining authored automatic exposure and TAA.
    pub fn capture(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        request: &CaptureRequest,
    ) -> Result<Capture> {
        ensure!(request.settle_frames <= 1024, "Settle frame limit is 1024");
        self.reset_capture(renderer)?;
        for _ in 0..request.settle_frames.max(4) {
            self.render(renderer, frame, &request.camera, request.mode)?;
        }
        renderer.capture_completed(request)
    }

    /// Restart reproducible capture state on this allocation, including particles.
    pub fn reset_capture(&mut self, renderer: &mut Renderer) -> Result<()> {
        self.wait_probe_gi(renderer)?;
        for _ in 0..256 {
            let stats = renderer
                .refine_native_textures()?
                .context("native capture residency")?;
            if stats.pending_textures == 0 {
                break;
            }
            ensure!(
                stats.uploaded_bytes > 0,
                "capture texture residency made no progress"
            );
        }
        ensure!(
            renderer
                .native_texture_stats()
                .is_some_and(|stats| stats.pending_textures == 0),
            "capture texture residency did not settle"
        );
        std::mem::take(&mut self.effects).destroy(renderer)?;
        renderer.reset_capture_history()?;
        Ok(())
    }

    /// Release view-owned particles while retaining the renderer's native scene.
    pub fn destroy(self, renderer: &mut Renderer) -> Result<()> {
        self.effects.destroy(renderer)
    }
}

/// Probe records applied per presented frame (8 MB): a full 32k-probe volume
/// lands over eight frames instead of stalling one.
const PROBES_PER_FRAME: usize = 4096;

/// Queues a delivered update; one that rewrites every probe supersedes the
/// updates still waiting.
fn queue_probe_gi(updates: &mut std::collections::VecDeque<ProbeUpdate>, update: ProbeUpdate) {
    if update.len() == update.probes() {
        updates.clear();
    }
    updates.push_back(update);
}

/// Applies up to `budget` queued probe records on the render thread, in order,
/// and reports what that cost.
fn apply_probe_gi(
    renderer: &mut Renderer,
    updates: &mut std::collections::VecDeque<ProbeUpdate>,
    budget: usize,
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let mut applied = 0;
    while applied < budget
        && let Some(mut update) = updates.pop_front()
    {
        if update.len() > budget - applied {
            let slice = update.split_off(budget - applied);
            updates.push_front(update);
            update = slice;
        }
        renderer.update_probe_surfaces(&update)?;
        applied += update.len();
    }
    eprintln!(
        "ara-runtime-render: applied {applied} probe GI records ({} queued) in {:.2} ms",
        updates.iter().map(ProbeUpdate::len).sum::<usize>(),
        started.elapsed().as_secs_f64() * 1e3
    );
    Ok(())
}

fn texture_config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1024 * 1024 * 1024,
        upload_bytes: 16 * 1024 * 1024,
    }
}

fn content_key(prepared: &Prepared) -> u64 {
    let mut key = crate::FNV;
    for geometry in &prepared.scene.geometry {
        key = crate::fnv(key, &geometry.key);
    }
    for instance in &prepared.scene.instances {
        key = crate::fnv(key, bytemuck::bytes_of(&instance.world));
    }
    key = crate::fnv(key, bytemuck::cast_slice(&prepared.scene.materials));
    key = crate::fnv(key, bytemuck::cast_slice(&prepared.scene.environment));
    for (digest, srgb) in prepared.textures.content_keys() {
        key = crate::fnv(key, &digest);
        key = crate::fnv(key, &[u8::from(srgb)]);
    }
    key
}
