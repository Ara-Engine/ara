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
mod batch;
pub(crate) use batch::Submitted;
mod fade;
mod prepared;
mod readiness;
pub use prepared::RasterContent;
#[cfg(test)]
mod tests;

/// Content-boundary result; ordinary movement keeps native allocations resident.
#[derive(Debug, Default, Clone, Copy)]
pub struct RasterUpdate {
    /// A complete geometry/material/texture generation was adopted.
    pub rebuilt: bool,
    /// Root transforms and skin geometries updated in place.
    pub updated: usize,
}

struct Preparation {
    snapshot_changed: bool,
    replacement: Option<(Prepared, u64, ara_render::NativeContent)>,
    scene_changed: bool,
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
    /// Delivered probe updates and how many of each one's records are applied.
    gi_updates: std::collections::VecDeque<(ProbeUpdate, usize)>,
    /// Emissive surfaces of the current generation and their lights, uploaded
    /// after the scene's own lights.
    emitters: Option<crate::emitters::Emitters>,
    /// The emitter search for the current content, running off this thread.
    emitter_search: crate::emitters::Search,
    emitter_lights: Vec<ara_render::scene::Light>,
    cache: PathBuf,
    key: u64,
    pending_gi: bool,
    look: Option<FrameEnvironment>,
    /// The colour lookup table the renderer holds (`FrameEnvironment::lut`).
    lut: Option<std::sync::Arc<ara_components::post::ColorLut>>,
    fades: fade::Fades,
    placeholder_changes: ara_render::instanced::Changes,
    /// Reflection captures: authored, or placed over the current generation.
    captures: crate::captures::Placement,
    /// Reflection-probe capture jobs and their retained state (`crate::probes`).
    probes: crate::probes::ProbeCaptures,
}

impl RasterView {
    /// A rejected preparation discards its CPU snapshot. Interactive callers
    /// must reconstruct it on their content worker, keeping last-good GPU data.
    pub fn has_prepared_content(&self) -> bool {
        self.prepared.is_some()
    }
    /// The bake's allocation metadata is sufficient to reserve resources before
    /// its records arrive. Called by the owning presenter during startup only.
    pub(crate) fn preallocate_probe_resources(&mut self, renderer: &mut Renderer) -> Result<bool> {
        let Some(gi) = &self.gi else { return Ok(true) };
        match gi.layout() {
            Some(layout) => renderer.preallocate_probe_volume(layout, PROBES_PER_FRAME),
            None => Ok(!gi.is_pending()),
        }
    }
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
            emitter_search: Default::default(),
            emitter_lights: Vec::new(),
            cache,
            key: crate::FNV,
            pending_gi: false,
            look: None,
            lut: None,
            fades: Default::default(),
            placeholder_changes: Default::default(),
            captures: Default::default(),
            probes: Default::default(),
        }
    }

    /// This view's reflection-probe capture jobs (`render.probes.query`).
    pub fn probes(&self) -> &crate::probes::ProbeCaptures {
        &self.probes
    }
    /// This view's reflection-probe capture jobs (`render.probe.recapture` and
    /// `render.probe.cancel`); bind the scene first (`ProbeCaptures::set_scene`).
    pub fn probes_mut(&mut self) -> &mut crate::probes::ProbeCaptures {
        &mut self.probes
    }
    /// Retires this view's probe work without rendering it again (its scene was
    /// replaced): see `ProbeCaptures::retire`. Bounded, no waits; call each owner tick
    /// after cancelling the view's jobs until it returns false.
    pub fn poll_probe_retirement(
        &mut self,
        renderer: &mut ara_render::Renderer,
    ) -> anyhow::Result<bool> {
        Ok(self.probes.retire(renderer))
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
        self.emitter_search.is_busy()
            || self.fades.is_active()
            || self.probe_gi_pending()
            || self.effects.is_pending()
    }

    /// GI acquisition/delivery only; emitters and model fades have their own work.
    pub(crate) fn probe_gi_pending(&self) -> bool {
        self.gi.as_ref().is_some_and(|gi| {
            self.pending_gi
                || gi.is_pending()
                || self.mobility.settling()
                || !self.gi_updates.is_empty()
        })
    }

    /// Installs a finished emitter search (waiting for the running one when
    /// `wait`); a queued rerun starts from the current content.
    fn adopt_emitters(&mut self, wait: bool) -> Result<()> {
        let Some(prepared) = &self.prepared else {
            return Ok(());
        };
        if let Some(emitters) = self.emitter_search.poll(wait, || emitter_job(prepared))? {
            self.emitters = (!emitters.is_empty()).then_some(emitters);
        }
        Ok(())
    }

    /// Re-runs the emitter search when an edit changed emission (a lamp's
    /// strength, colour or emissive switch); the old lights stay until it lands.
    fn search_emission_edits(&mut self) -> Result<()> {
        let Some(prepared) = &self.prepared else {
            return Ok(());
        };
        let changed = &prepared.changes().materials;
        if !changed.is_empty()
            && crate::emitters::emission_edited(
                self.emitters.as_ref(),
                &prepared.scene,
                |texture, decode| prepared.textures.mean_texel(texture, decode),
                changed,
            )
        {
            self.emitter_search.request(|| emitter_job(prepared))?;
        }
        Ok(())
    }

    /// Fade newly arrived model asset IDs over twelve presented frames, including
    /// prefab foliage. Existing generations on a reimport must not enter this set.
    /// Call only after their content publication succeeds. Motion reserves one
    /// material per root primitive, so this never changes a sibling's opacity.
    pub fn fade_models(&mut self, arrivals: &std::collections::BTreeSet<String>) -> Result<()> {
        let prepared = self.prepared.as_ref().context("native raster content")?;
        self.fades.begin(prepared, arrivals);
        Ok(())
    }

    /// Update loading/failed bounds of unpublished foliage parts from read-only
    /// status, without extracting geometry or changing decoded-model revisions.
    pub fn style_unpublished(
        &mut self,
        renderer: &mut Renderer,
        is_pending: impl Fn(&str) -> bool,
    ) -> Result<()> {
        let Some(prepared) = &mut self.prepared else {
            return Ok(());
        };
        self.placeholder_changes.clear();
        for (asset, indices) in &prepared.unpublished_materials {
            let pending = is_pending(asset);
            let color = if pending {
                crate::PENDING_MODEL_COLOR
            } else {
                [1., 0., 1., 1.]
            };
            for &index in indices {
                let material = &mut prepared.scene.materials[index].pbr;
                if material.base_color != color {
                    material.base_color = color;
                    material.flags[0] = if pending { 2 } else { 0 };
                    self.placeholder_changes.materials.push(index);
                }
            }
        }
        if !self.placeholder_changes.materials.is_empty() {
            with_emitter_lights(&mut prepared.scene, &self.emitter_lights, |scene| {
                renderer.update_native_changed(scene, &self.placeholder_changes)
            })?;
        }
        Ok(())
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
        self.prepare_inner(
            renderer,
            frame,
            plan,
            look,
            resolve,
            Preparation {
                snapshot_changed: true,
                replacement: None,
                scene_changed: false,
            },
        )
    }

    /// Publish a CPU-prepared content snapshot at the device's frame boundary.
    /// Build on a worker and preallocate before an asynchronous upload scope.
    /// Synchronous callers drain preparation here. Failure retains last-good.
    pub fn publish_content(
        &mut self,
        renderer: &mut Renderer,
        candidate: RasterContent,
        plan: RasterPlan,
    ) -> Result<RenderFrame> {
        self.publish_content_inner(renderer, candidate, plan, false)
    }

    /// Publish a different scene, resetting its GI, particles and exposure only
    /// after native publication succeeds. Asset arrivals within a scene instead
    /// use `publish_content`, preserving their live simulation and lighting.
    pub fn publish_scene(
        &mut self,
        renderer: &mut Renderer,
        candidate: RasterContent,
        plan: RasterPlan,
    ) -> Result<RenderFrame> {
        self.publish_content_inner(renderer, candidate, plan, true)
    }

    fn publish_content_inner(
        &mut self,
        renderer: &mut Renderer,
        mut candidate: RasterContent,
        plan: RasterPlan,
        scene_changed: bool,
    ) -> Result<RenderFrame> {
        if let Err(error) = candidate.native.reserve_shadow_quality(&plan.quality) {
            candidate.destroy(renderer)?;
            return Err(error);
        }
        loop {
            match candidate.preallocate(renderer) {
                Ok(true) => break,
                Ok(false) if !renderer.gpu().deferring_completion() => {
                    // Synchronous callers explicitly publish a complete generation.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                result => {
                    candidate.destroy(renderer)?;
                    return match result {
                        Err(error) => Err(error),
                        _ => anyhow::bail!(
                            "preallocate native content before an asynchronous upload scope"
                        ),
                    };
                }
            }
        }
        let RasterContent {
            frame,
            content,
            prepared,
            key,
            native,
        } = candidate;
        let look = crate::environment(&frame);
        self.prepare_inner(
            renderer,
            &frame,
            plan,
            look,
            || Ok(content),
            Preparation {
                snapshot_changed: true,
                replacement: Some((prepared, key, native)),
                scene_changed,
            },
        )?;
        Ok(frame)
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
        self.prepare_inner(
            renderer,
            frame,
            plan,
            look,
            resolve,
            Preparation {
                snapshot_changed: false,
                replacement: None,
                scene_changed: false,
            },
        )
    }

    fn prepare_inner(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        plan: RasterPlan,
        look: FrameEnvironment,
        resolve: impl FnOnce() -> Result<Content>,
        preparation: Preparation,
    ) -> Result<RasterUpdate> {
        ara_gpu::cpu_scope!("RasterView.prepare");
        let Preparation {
            snapshot_changed,
            replacement,
            scene_changed,
        } = preparation;
        let result = (|| {
            let change = if replacement.is_some() {
                FrameChange::Rebuild
            } else if snapshot_changed || self.prepared.is_none() {
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
                let (mut prepared, key) = match replacement {
                    Some((mut prepared, key, native)) => {
                        prepared.scene = renderer.publish_native_content(native)?;
                        (prepared, Some(key))
                    }
                    None => {
                        let mut prepared = native::prepare(frame, content, config)?;
                        prepared.enable_motion();
                        let textures = prepared.textures.fork_sources(config)?;
                        renderer.replace_native_scene(&prepared.scene, Box::new(textures))?;
                        (prepared, None)
                    }
                };
                if scene_changed {
                    // Cross-scene identities may alias (e_player, e_dust, etc.).
                    // None of their temporal state or late probe deliveries belong
                    // to the successfully published replacement.
                    std::mem::take(&mut self.effects).destroy(renderer)?;
                    self.mobility = Default::default();
                    self.gi_updates.clear();
                    if let Some(gi) = &mut self.gi {
                        gi.reset_scene();
                    }
                    renderer.set_probe_surfaces(None)?;
                    // With the old volume gone this queues only exposure/history
                    // resets; it neither drains nor clears the previous GI pool.
                    renderer.reset_capture_history()?;
                    self.fades = Default::default();
                    self.emitter_lights.clear();
                }
                prepared.enable_motion();
                self.key = key.unwrap_or_else(|| content_key(&prepared));
                let arrivals = self
                    .prepared
                    .as_ref()
                    .into_iter()
                    .filter(|_| !scene_changed)
                    .flat_map(|old| old.unpublished_materials.keys())
                    .filter(|asset| {
                        prepared
                            .scene
                            .instances
                            .iter()
                            .enumerate()
                            .any(|(i, _)| prepared.model_source(i) == Some(asset.as_str()))
                    })
                    .cloned()
                    .collect();
                self.fades.rebind(&prepared, &arrivals);
                // Emitter lights arrive from a worker; until then emitters light
                // nothing (their emission is out of the probes too).
                self.emitters = None;
                self.emitter_search.restart(emitter_job(&prepared))?;
                self.prepared = Some(prepared);
                self.pending_gi = true;
                update.rebuilt = true;
            } else if let FrameChange::Ready { poses, skins } = change {
                update.updated = poses + skins;
                if snapshot_changed {
                    self.search_emission_edits()?;
                }
            }
            renderer.resize_upscaled(plan.render_size, plan.display_size)?;
            if *renderer.quality() != plan.quality {
                ara_gpu::cpu_scope!("RasterView.apply_quality");
                renderer.set_quality(plan.quality)?;
            }
            let raster_look = look.raster();
            if *renderer.look() != raster_look {
                ara_gpu::cpu_scope!("RasterView.apply_look");
                renderer.set_look(raster_look)?;
            }
            crate::apply_lut(renderer, frame, &look, &mut self.lut)?;
            // Reflection captures: authored, else placed over this generation's content.
            if let Some(prepared) = &self.prepared {
                let (limit, automatic) = plan
                    .quality
                    .reflection_captures
                    .map_or((0, false), |r| (r.captures as usize, r.automatic));
                let specs = self
                    .captures
                    .specs(frame, &prepared.scene, self.key, limit, automatic);
                renderer.set_reflection_captures(&specs)?;
            }
            self.probes.tick(renderer, frame, self.key);
            self.adopt_emitters(false)?;
            let prepared = self.prepared.as_mut().context("native raster content")?;
            if snapshot_changed {
                self.fades.refresh(&mut prepared.scene);
            }
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
                ara_gpu::cpu_scope!("RasterView.probe_control");
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
        self.render_recorded(renderer, frame, camera, mode, &mut [0; 2])
    }

    fn render_recorded(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
        boundaries: &mut [usize; 2],
    ) -> Result<GpuFrameCosts> {
        ara_gpu::cpu_scope!("RasterView.render");
        ensure!(
            renderer.is_native(),
            "raster content must be prepared before rendering"
        );
        let look = self.look.context("raster look must be prepared")?;
        let prepared = self.prepared.as_mut().context("native raster content")?;
        with_emitter_lights(&mut prepared.scene, &self.emitter_lights, |scene| {
            self.fades.apply(scene, renderer)
        })?;
        let lit = matches!(mode, ViewMode::Lit);
        renderer.set_deferred_post(lit && !frame.effects.is_empty());
        renderer.render_view(
            camera,
            look.manual_exposure(frame.environment.exposure),
            mode,
        )?;
        boundaries[0] = renderer.gpu().deferred_submission_count();
        boundaries[1] = boundaries[0];
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
            boundaries[1] = renderer.gpu().deferred_submission_count();
            if !frame.effects.is_empty() {
                renderer.finish(color.image, color.sampled_index, color.state)?;
            }
        }
        let textures = renderer
            .native_texture_stats()
            .context("native residency statistics")?;
        self.fades.presented();
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

    /// Draw another camera over the retained native scene, without rendering the
    /// primary camera. Arrival fades belong to the shared scene and advance only
    /// after a successful presentation; the renderer owns each camera's history.
    pub fn render_secondary(
        &mut self,
        renderer: &mut Renderer,
        view: &mut ara_render::ViewId,
        request: &ara_render::ViewRequest<'_>,
    ) -> Result<ara_render::ViewOutput> {
        self.prepare_secondary_frame(renderer)?;
        let output = renderer.render_secondary(view, request)?;
        self.secondary_frame_submitted();
        Ok(output)
    }

    /// Apply shared model-arrival factors before the secondary camera copies its
    /// native rows. Call inside that frame's ordered preparation scope.
    pub fn prepare_secondary_frame(&mut self, renderer: &mut Renderer) -> Result<()> {
        let prepared = self.prepared.as_mut().context("native raster content")?;
        with_emitter_lights(&mut prepared.scene, &self.emitter_lights, |scene| {
            self.fades.apply(scene, renderer)
        })
    }

    /// Advance arrival fades only for a successfully submitted secondary frame,
    /// never while polling a retained image or rejecting/coalescing a request.
    pub fn secondary_frame_submitted(&mut self) {
        self.fades.presented();
    }

    /// Wait only at an explicit capture/measurement boundary, never in live rendering.
    pub fn wait_probe_gi(&mut self, renderer: &mut Renderer) -> Result<()> {
        while self.emitter_search.is_busy() {
            self.adopt_emitters(true)?;
        }
        // A headless capture renders directly after this boundary, without a
        // further prepare. Publish newly adopted lights before its first frame.
        if let Some(prepared) = &mut self.prepared {
            if let Some(emitters) = &mut self.emitters {
                let budget = (renderer.quality().emitter_lights as usize).min(
                    ara_render::scene::MAX_LOCAL_LIGHTS.saturating_sub(prepared.scene.lights.len()),
                );
                if let Some(lights) = emitters.update(&prepared.scene, budget) {
                    self.emitter_lights = lights;
                }
            }
            with_emitter_lights(&mut prepared.scene, &self.emitter_lights, |scene| {
                renderer.update_native_changed(scene, &ara_render::instanced::Changes::default())
            })?;
        }
        if let Some(gi) = &mut self.gi {
            while let Some(update) = gi.poll(true) {
                queue_probe_gi(&mut self.gi_updates, update);
            }
        }
        renderer.complete_probe_preparation()?;
        apply_probe_gi(renderer, &mut self.gi_updates, usize::MAX)?;
        Ok(())
    }

    /// Capture this presentation, retaining authored automatic exposure and TAA.
    /// Complete lazy effects before a capture's temporal settle window. Live
    /// callers only poll; this explicit boundary may wait for the compiler.
    pub fn wait_capture_effects(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        camera: &CameraSpec,
        mode: ViewMode,
    ) -> Result<()> {
        if matches!(mode, ViewMode::Lit) {
            self.effects.wait_pipelines(renderer, frame)?;
            while self.effects.is_pending() {
                self.render(renderer, frame, camera, mode)?;
            }
            // Hybrid ray-query PSOs compile off-thread after the native ones, which
            // frames advance; until they land, frames keep the raster path. Bounded:
            // a stuck preparation leaves the capture raster rather than hanging it.
            let started = std::time::Instant::now();
            while renderer.ray_pipelines_pending() && started.elapsed().as_secs() < 120 {
                self.render(renderer, frame, camera, mode)?;
            }
        }
        renderer.reset_capture_history()?;
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
        self.wait_capture_effects(renderer, frame, &request.camera, request.mode)?;
        for _ in 0..request.settle_frames.max(4) {
            self.render(renderer, frame, &request.camera, request.mode)?;
        }
        renderer.capture_completed(request)
    }

    /// Restart reproducible capture state on this allocation, including particles.
    pub fn reset_capture(&mut self, renderer: &mut Renderer) -> Result<()> {
        if let Some(prepared) = &mut self.prepared {
            with_emitter_lights(&mut prepared.scene, &self.emitter_lights, |scene| {
                self.fades.finish(scene, renderer)
            })?;
        }
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

/// An emitter search of `prepared`'s current content.
fn emitter_job(prepared: &Prepared) -> crate::emitters::Job {
    ara_gpu::cpu_scope!("RasterView.capture_emitters");
    crate::emitters::Job::new(
        &prepared.scene,
        &crate::gi::emission(prepared),
        |texture, side, decode| prepared.textures.texels(texture, side, decode),
    )
}

/// Runs a native `upload` of `scene` with the emitter `lights` after its own, so
/// every upload (prepare, arrival fades, placeholder styling) keeps them; the
/// scene's own list is restored afterwards.
fn with_emitter_lights<T>(
    scene: &mut ara_render::instanced::Scene,
    lights: &[ara_render::scene::Light],
    upload: impl FnOnce(&mut ara_render::instanced::Scene) -> Result<T>,
) -> Result<T> {
    let own = scene.lights.len();
    scene.lights.extend_from_slice(lights);
    let result = upload(scene);
    scene.lights.truncate(own);
    result
}

/// Queues a delivered update; one that rewrites every probe, or begins a new
/// layout, supersedes the updates still waiting.
fn queue_probe_gi(
    updates: &mut std::collections::VecDeque<(ProbeUpdate, usize)>,
    update: ProbeUpdate,
) {
    // A new pool arrives in stages, none of them whole: a changed layout
    // supersedes what is queued for the old one.
    let stale = updates
        .back()
        .is_some_and(|(queued, _)| queued.layout != update.layout);
    if update.len() == update.probes() || stale {
        updates.clear();
    }
    updates.push_back((update, 0));
}

/// Applies up to `budget` queued probe records on the render thread, in order,
/// uploading ranges straight from the delivered updates, and reports what that
/// cost (the next frame scatters them).
fn apply_probe_gi(
    renderer: &mut Renderer,
    updates: &mut std::collections::VecDeque<(ProbeUpdate, usize)>,
    budget: usize,
) -> Result<()> {
    if updates.is_empty() || renderer.probe_preparation_pending() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let mut applied = 0;
    while applied < budget
        && let Some((update, next)) = updates.front_mut()
    {
        let end = *next + (update.len() - *next).min(budget - applied);
        renderer.update_probe_records(update, *next..end)?;
        applied += end - *next;
        *next = end;
        if *next == update.len() {
            updates.pop_front();
        }
    }
    eprintln!(
        "ara-runtime-render: applied {applied} probe GI records ({} queued) in {:.2} ms",
        updates
            .iter()
            .map(|(u, next)| u.len() - next)
            .sum::<usize>(),
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
