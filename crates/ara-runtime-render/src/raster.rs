//! Native raster view shared by the editor, Play and installed players.
use crate::{
    GpuFrameCosts, Renderer,
    fidelity::native::{self, Content, FrameChange, NativeConfig, Prepared},
    vfx,
};
use anyhow::{Context, Result, ensure};
use ara_render::capture::{Capture, CaptureRequest, ViewMode};
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
        self.gi
            .as_ref()
            .is_some_and(|gi| gi.is_pending() || self.mobility.settling())
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
        ara_gpu::cpu_scope!("RasterView.prepare");
        let change = self
            .prepared
            .as_mut()
            .map(|p| p.update_frame(frame))
            .transpose()?
            .unwrap_or(FrameChange::Rebuild);
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
        renderer.update_native(
            &self
                .prepared
                .as_ref()
                .context("native raster content")?
                .scene,
        )?;
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
            if let Some(surfaces) = gi.poll(false) {
                renderer.update_probe_surfaces(&surfaces)?;
            }
        }
        Ok(update)
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
        if let Some(surfaces) = self.gi.as_mut().and_then(|gi| gi.poll(true)) {
            renderer.update_probe_surfaces(&surfaces)?;
        }
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
