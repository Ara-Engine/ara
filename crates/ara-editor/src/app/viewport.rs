//! Same-device Scene quality seam. The raster view retains IDs and capture metadata.
use super::*;
use ara_render_fidelity::{
    FidelityView, FrameInput, PostSettings,
    environment::FrameEnvironment,
    presets::{Capabilities, FidelityPlan, Plan, RenderPath},
};
use ara_runtime_render::fidelity::native::{self, FrameChange, NativeConfig, Prepared};
use ara_runtime_render::{PresentationImage, fidelity::vfx::Effects};
mod content;
mod preview;
pub(super) use preview::{GamePreview, PreviewCamera};

#[derive(Clone, Copy)]
enum RasterWork {
    Live(u32),
    Submitted(u32),
    Capture(u32),
    Prepare,
}
#[derive(Default)]
pub(super) struct SceneView {
    raster: Option<ara_runtime_render::RasterView>,
    raster_frame: Option<ara_runtime::extract::RenderFrame>,
    raster_snapshot_dirty: bool,
    /// One submitted Scene frame; UI draws its previous completed color/ID pair.
    frame_pending: bool,
    fidelity: Option<FidelityView>,
    content: Option<crate::world::gltf::FidelityContent>,
    prepared: Option<Prepared>,
    effects: Option<Effects>,
    published: Option<PresentationImage>,
    plan: Option<FidelityPlan>,
    active: bool,
    revision: Option<(u64, u64, u64, u64)>,
    failed: Option<(u64, u64, u64, u64)>,
    tick: u32,
    remaining: u32,
    camera: Option<crate::state::Camera>,
    /// The fidelity view applies to the Lit mode only; debug views show the raster output.
    lit: bool,
    /// Transient typed drafts, projected through the same native extraction as commits.
    preview_view: Option<crate::world::DocumentView>,
    /// Pose already incorporated by the native patch path (no repeated extraction).
    patched_pose: Option<u64>,
    /// One CPU-only replacement at a time; newer imports coalesce until it finishes.
    content_job: Option<content::Job>,
    /// GPU allocations are prepared off-thread before entering the upload scope.
    content_ready: Option<content::Publication>,
    content_epoch: u64,
    /// Input actually reflected by the retained native frame. SceneSync can
    /// invalidate after this view already adopted the same publication.
    raster_input: Option<content::Key>,
}
impl SceneView {
    /// Recheck native input at the next render boundary. Notifications for an
    /// already adopted publication coalesce; changed input retains last-good GPU
    /// content until its replacement or in-place update succeeds.
    pub(super) fn invalidate(&mut self) {
        self.revision = None;
        self.failed = None;
        self.remaining = self.remaining.max(8);
        self.patched_pose = None;
    }
    /// Project typed Inspector drafts without authoring documents or baking meshes.
    pub(super) fn set_preview_view(&mut self, view: Option<crate::world::DocumentView>) {
        self.content_epoch = self.content_epoch.wrapping_add(1);
        self.preview_view = view;
        self.invalidate();
    }
    /// Update only `patch` on the retained native frame. A typed preview remains
    /// projected for later publication/resize rebuilds. False requests extraction.
    pub(super) fn apply_patch(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        committed: &crate::world::DocumentView,
        preview: Option<crate::world::DocumentView>,
        patch: &crate::world::Patch,
    ) -> ara_ui::Result<bool> {
        ara_gpu::cpu_scope!("SceneView.apply_patch");
        if !renderer.is_native() || self.failed.is_some() {
            return Ok(false);
        }
        let (Some(frame), Some(raster), Some(content)) =
            (&mut self.raster_frame, &mut self.raster, &mut self.content)
        else {
            return Ok(false);
        };
        let publication = content.snapshot_revision();
        if self
            .revision
            .is_some_and(|revision| publication != revision.3)
        {
            return Ok(false);
        }
        let store = editor.host.store();
        let settings = crate::world::gltf::render_settings(store.workspace())?;
        let choice = editor
            .state
            .render_quality
            .unwrap_or(settings.quality.into());
        let plan = Plan::resolve(
            choice,
            &Capabilities::of(renderer.gpu()),
            renderer.size(),
            ara_runtime_render::RenderOptions {
                ray_tracing: settings.ray_tracing,
            },
        )?;
        let RenderPath::Raster(_) = plan.path else {
            return Ok(false);
        };
        let target = preview.as_ref().unwrap_or(committed);
        if !content.patch(
            frame,
            target,
            store.workspace(),
            patch,
            editor.gesture.as_ref(),
            &editor.state.hidden,
        )? {
            return Ok(false);
        }
        // Upload once at render_raster's normal frame boundary, rather than once
        // per input event and again when that frame is drawn.
        for draw in &frame.draws {
            if patch.renamed.contains(&draw.address) {
                renderer.rename(draw.entity, draw.label.to_owned())?;
                raster.rename(draw.entity, &draw.label)?;
            }
        }
        if patch.materials || patch.lights {
            renderer.reset_temporal_history();
        }
        self.preview_view = preview;
        if !patch.is_empty() {
            self.content_epoch = self.content_epoch.wrapping_add(1);
        }
        self.raster_snapshot_dirty = true;
        self.patched_pose = pose_key(editor.gesture.as_ref());
        let revision = (
            store.revision(),
            editor.extraction_revision,
            editor.review_revision,
            publication,
        );
        self.revision = Some(revision);
        self.raster_input = Some(content::Key {
            revision,
            epoch: self.content_epoch,
            pose: self.patched_pose,
            scene: content::scene_key(editor),
        });
        self.remaining = self.remaining.max(8);
        Ok(true)
    }
    pub fn timings(&self) -> &[ara_gpu::commands::PassTiming] {
        self.fidelity
            .as_ref()
            .map(|view| view.timings())
            .unwrap_or(&[])
    }
    /// Settle a reproducible capture on the live allocation. Reset lighting history
    /// and the jitter/RNG sequence without holding a second view's GPU descriptors.
    pub(super) fn render_capture(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        renderer.wait_scene_frame()?;
        self.frame_pending = false;
        if super::scene_sync::native(editor, renderer) {
            self.lit = matches!(view_mode(&editor.state.view_mode), ViewMode::Lit);
            // Capture is an explicit blocking boundary. Drain superseded CPU
            // snapshots and pending GPU work until the requested view is ready;
            // live rendering instead retains the completed image and polls.
            loop {
                self.render_raster_inner(renderer, editor, view, RasterWork::Capture(1))?;
                if self.content_job.is_none() && self.content_ready.is_none() {
                    break;
                }
            }
        } else {
            self.render(renderer, editor, view, 1)?;
        }
        if !self.active {
            return Ok(());
        }
        if self.raster.is_some() && self.fidelity.is_none() {
            self.raster
                .as_mut()
                .ok_or("raster Scene view")?
                .reset_capture(renderer)?;
            return self.render_raster_inner(renderer, editor, view, RasterWork::Capture(settle));
        }
        self.fidelity
            .as_mut()
            .ok_or("Scene capture has no fidelity view")?
            .reset_accumulation(renderer.gpu_mut())?;
        self.tick = 0;
        self.render_fidelity(renderer, editor, view, settle)?;
        Ok(())
    }
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        let mode = view_mode(&editor.state.view_mode);
        self.lit = matches!(mode, ViewMode::Lit);
        let settings = crate::world::gltf::render_settings(editor.host.store().workspace())?;
        let options = ara_runtime_render::RenderOptions {
            ray_tracing: settings.ray_tracing,
        };
        let choice = editor
            .state
            .render_quality
            .unwrap_or(settings.quality.into());
        let plan = Plan::resolve(
            choice,
            &Capabilities::of(renderer.gpu()),
            renderer.size(),
            options,
        )?;
        if matches!(plan.path, RenderPath::Raster(_)) {
            if let Err(error) = self.render_raster(renderer, editor, view, settle) {
                eprintln!("Scene raster keeps its last good presentation: {error}");
                self.remaining = 0;
                self.frame_pending = renderer.scene_frame_pending();
                self.raster_input = None;
                self.failed = Some((
                    editor.host.store().revision(),
                    editor.extraction_revision,
                    editor.review_revision,
                    self.content
                        .as_ref()
                        .map_or(0, |content| content.revision()),
                ));
                // Scene content can fail before its first publication. Keep the
                // editor alive and leave unwritten working targets unattached.
                self.active = if renderer.scene_frame_async() {
                    renderer.presentation_ready()
                } else {
                    self.published.is_some()
                };
            }
            return Ok(());
        }
        if !renderer.poll_scene_frame()? {
            self.frame_pending = true;
            return Ok(());
        }
        self.frame_pending = false;
        self.content_job = None;
        if let Some(candidate) = self.content_ready.take() {
            candidate.scene.raster.destroy(renderer)?;
        }
        if self.preview_view.is_some() {
            self.set_preview_view(None);
        }
        if let Some(raster) = self.raster.take() {
            raster.destroy(renderer)?;
            renderer.clear_native()?;
            self.raster_frame = None;
            self.raster_input = None;
            self.invalidate();
        }
        renderer.render_view(&editor.state.camera.spec(), 0.0015, mode)?;
        self.active = false;
        if !lit_fidelity(editor) {
            return Ok(());
        }
        if let Err(error) = self.render_fidelity(renderer, editor, view, settle) {
            eprintln!("Scene fidelity keeps its last good presentation: {error}");
            self.remaining = 0;
            self.revision = Some((
                editor.host.store().revision(),
                editor.extraction_revision,
                editor.review_revision,
                self.content
                    .as_ref()
                    .map_or(0, |content| content.revision()),
            ));
        }
        Ok(())
    }
    fn render_raster(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        self.render_raster_inner(renderer, editor, view, RasterWork::Live(settle))
    }
    fn render_raster_inner(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        work: RasterWork,
    ) -> ara_ui::Result<()> {
        ara_gpu::cpu_scope!("SceneView.render_raster");
        let wait_for_content = matches!(work, RasterWork::Capture(_));
        if wait_for_content {
            renderer.wait_scene_frame()?;
        } else if !renderer.poll_scene_frame()? {
            self.frame_pending = true;
            return Ok(());
        }
        self.frame_pending = false;
        if renderer.scene_frame_async() {
            self.active = renderer.presentation_ready();
        }
        let settle = match work {
            RasterWork::Live(settle)
            | RasterWork::Submitted(settle)
            | RasterWork::Capture(settle) => settle,
            RasterWork::Prepare => 0,
        };
        let store = editor.host.store();
        let settings = crate::world::gltf::render_settings(store.workspace())?;
        let choice = editor
            .state
            .render_quality
            .unwrap_or(settings.quality.into());
        let plan = Plan::resolve(
            choice,
            &Capabilities::of(renderer.gpu()),
            renderer.size(),
            ara_runtime_render::RenderOptions {
                ray_tracing: settings.ray_tracing,
            },
        )?;
        let RenderPath::Raster(plan) = plan.path else {
            return Ok(());
        };
        if let Some(fidelity) = self.fidelity.take() {
            fidelity.destroy(renderer.gpu_mut())?;
            self.prepared = None;
            self.plan = None;
            self.revision = None;
        }
        if let Some(effects) = self.effects.take() {
            effects.destroy(renderer.gpu_mut())?;
        }
        let content = match &mut self.content {
            Some(content) => content,
            slot @ None => slot.insert(crate::world::gltf::FidelityContent::new(
                editor.host.root(),
            )?),
        };
        let revision = (
            store.revision(),
            editor.extraction_revision,
            editor.review_revision,
            content.revision(),
        );
        let key = content::Key {
            revision,
            epoch: self.content_epoch,
            pose: pose_key(editor.gesture.as_ref()),
            scene: content::scene_key(editor),
        };
        if self.failed == Some(revision)
            && matches!(work, RasterWork::Live(_) | RasterWork::Submitted(_))
        {
            // Retry only when content/publication/preview changes, rather than
            // allocating a failed generation on every UI frame.
            return Ok(());
        }
        if self.revision != Some(revision)
            && self.raster_frame.is_some()
            && self.raster_input.is_some_and(|shown| shown.same_input(key))
        {
            // A settled ModelCache/extraction notification may arrive after this
            // view's own publication. Acknowledge it without another snapshot.
            self.revision = Some(revision);
            self.patched_pose = key.pose;
        }
        let changed = self.revision != Some(revision)
            || (editor.gesture.is_some() && self.patched_pose != pose_key(editor.gesture.as_ref()))
            || self.raster_frame.is_none();
        content::poll(
            &mut self.content_job,
            &mut self.content_ready,
            key,
            renderer,
            wait_for_content,
        )?;
        if changed && self.content_job.is_none() && self.content_ready.is_none() {
            content::queue_reason(self.revision, key);
            self.content_job = Some(content::Job::start(
                key,
                content,
                editor,
                self.preview_view.as_ref().unwrap_or(view),
            )?);
        }
        if matches!(work, RasterWork::Live(_)) {
            let ready = self.raster_frame.is_some()
                || self
                    .content_ready
                    .as_ref()
                    .is_some_and(|candidate| candidate.ready);
            if !ready {
                return Ok(());
            }
            // CPU snapshotting is outside the scope. All GPU adoption, probe
            // delivery, feature composition and post share its completion token.
            let result = renderer.submit_scene_frame(|renderer| {
                self.render_raster_inner(renderer, editor, view, RasterWork::Submitted(settle))
                    .map_err(|error| anyhow::anyhow!("{error}"))
            });
            self.frame_pending = renderer.scene_frame_pending();
            self.active = renderer.presentation_ready();
            return result.map(|_| ()).map_err(Into::into);
        }
        if self
            .content_ready
            .as_ref()
            .is_some_and(|candidate| candidate.ready)
        {
            let candidate = self
                .content_ready
                .take()
                .ok_or("Scene content publication")?;
            let prepared = candidate.scene;
            self.failed = Some(revision);
            let raster = self.raster.get_or_insert_with(|| {
                ara_runtime_render::RasterView::new(editor.host.root().join(".ara/gpu"))
            });
            let frame = raster.publish_content(renderer, prepared.raster, plan)?;
            *content = prepared.content;
            let arrivals = content.take_arrivals();
            if !arrivals.is_empty() {
                raster.fade_models(&arrivals)?;
            }
            self.raster_frame = Some(frame);
            self.raster_snapshot_dirty = false;
            self.revision = Some((
                revision.0,
                revision.1,
                revision.2,
                content.snapshot_revision(),
            ));
            self.raster_input = self
                .revision
                .map(|revision| content::Key { revision, ..key });
            self.patched_pose = key.pose;
            self.failed = None;
            self.remaining = 64;
        }
        if wait_for_content && (self.content_job.is_some() || self.content_ready.is_some()) {
            // Explicit capture drains preparation. Live/preview calls only poll
            // and keep displaying the last completed scene while the worker runs.
            std::thread::sleep(std::time::Duration::from_millis(1));
            return Ok(());
        }
        if self.raster_frame.is_none() {
            return Ok(());
        }
        {
            let frame = self.raster_frame.as_mut().ok_or("raster frame")?;
            if let Some(changed) =
                content.style_pending(frame, self.preview_view.as_ref().unwrap_or(view))
            {
                self.raster_snapshot_dirty |= changed;
                self.remaining = self.remaining.max(8);
                self.raster
                    .as_mut()
                    .ok_or("raster view")?
                    .style_unpublished(renderer, |asset| content.model_is_pending(asset))?;
            }
            let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
            let raster = self.raster.as_mut().ok_or("raster view")?;
            let resolve = || content.content(frame, store.workspace(), store.schema());
            if self.raster_snapshot_dirty {
                raster.prepare(renderer, frame, plan, look, resolve)?;
            } else {
                raster.prepare_unchanged(renderer, frame, plan, look, resolve)?;
            }
            self.raster_snapshot_dirty = false;
        }
        self.failed = None;
        if matches!(work, RasterWork::Prepare) {
            return Ok(());
        }
        if self.camera != Some(editor.state.camera) {
            self.remaining = self.remaining.max(8);
            self.camera = Some(editor.state.camera);
        }
        let raster = self.raster.as_mut().ok_or("raster view")?;
        let frame = self.raster_frame.as_ref().ok_or("raster frame")?;
        for _ in 0..settle.max(1) {
            raster.render(
                renderer,
                frame,
                &editor.state.camera.spec(),
                view_mode(&editor.state.view_mode),
            )?;
            self.remaining = self.remaining.saturating_sub(1);
            if renderer
                .native_texture_stats()
                .is_some_and(|stats| stats.pending_textures > 0)
            {
                self.remaining = self.remaining.max(1);
            }
        }
        if matches!(work, RasterWork::Submitted(_)) {
            return Ok(());
        }
        let output = renderer.output();
        let size = renderer.size();
        PresentationImage::publish(&mut self.published, renderer.gpu_mut(), output, size)?;
        self.failed = None;
        self.active = true;
        Ok(())
    }
    fn render_fidelity(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
        settle: u32,
    ) -> ara_ui::Result<()> {
        let size = renderer.size();
        let choice = editor
            .state
            .render_quality
            .unwrap_or(crate::world::gltf::render_quality(
                editor.host.store().workspace(),
            )?);
        // The ray-traced tiers until the D27 cut-over (native raster geometry).
        let options = ara_render_fidelity::presets::RenderOptions { ray_tracing: true };
        let plan = Plan::resolve(choice, &Capabilities::of(renderer.gpu()), size, options)?;
        let RenderPath::Fidelity(settings) = plan.path else {
            return Ok(());
        };
        self.active = true;
        if self.content.is_none() {
            self.content = Some(crate::world::gltf::FidelityContent::new(
                editor.host.root(),
            )?);
        }
        let content = self.content.as_mut().ok_or("fidelity sources")?;
        let revision = (
            editor.host.store().revision(),
            editor.extraction_revision,
            editor.review_revision,
            content.revision(),
        );
        if self.camera != Some(editor.state.camera) {
            self.remaining = self.remaining.max(8);
            self.camera = Some(editor.state.camera);
        }
        let reset = self.revision != Some(revision) || editor.gesture.is_some();
        let resized = self.plan != Some(settings);
        if reset || resized {
            self.remaining = 64;
        }
        let mut replaced = false;
        if (self.fidelity.is_none() || reset)
            && (self.failed != Some(revision) || editor.gesture.is_some())
        {
            // A rejected generation retries when its inputs change, rather than
            // repeatedly decoding it while the last good image is still displayed.
            self.failed = Some(revision);
            let store = editor.host.store();
            let frame = content.frame(
                view,
                store.workspace(),
                store.schema(),
                editor.gesture.as_ref(),
                &editor.state.hidden,
            )?;
            let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
            let rebuild = self
                .prepared
                .as_mut()
                .map(|prepared| prepared.update_frame(&frame))
                .transpose()?
                .is_none_or(|change| change == FrameChange::Rebuild);
            if rebuild {
                let assets = content.content(&frame, store.workspace(), store.schema())?;
                let mut prepared = native::prepare(&frame, assets, texture_config())?;
                prepared.enable_motion();
                let bank = prepared.textures.fork_sources(texture_config())?;
                if let Some(fidelity) = &mut self.fidelity {
                    fidelity.replace_native_scene(renderer.gpu_mut(), &prepared.scene, bank)?;
                } else {
                    self.fidelity = Some(FidelityView::new_native(
                        renderer.gpu_mut(),
                        &prepared.scene,
                        bank,
                        settings.settings(look.post(PostSettings::default())),
                        None,
                    )?);
                }
                self.prepared = Some(prepared);
                replaced = true;
            } else if let (Some(fidelity), Some(prepared)) = (&mut self.fidelity, &self.prepared) {
                fidelity.update_native_scene(renderer.gpu_mut(), &prepared.scene)?;
            }
            self.fidelity
                .as_mut()
                .ok_or("fidelity allocation")?
                .set_environment(&look)?;
            let sun = frame.sun.map_or((Vec3::Y, Vec3::ZERO), |(direction, lux)| {
                (Vec3::from_array(direction), Vec3::from_array(lux))
            });
            let sun = (sun.0, look.ground_sun(sun.0, sun.1));
            if self.effects.is_none() {
                self.effects = Some(Effects::new(
                    renderer.gpu_mut(),
                    &[],
                    settings.render_size,
                    sun,
                )?);
            }
            self.effects.as_mut().ok_or("effect allocation")?.update(
                renderer.gpu_mut(),
                &frame.effects,
                sun,
            )?;
            self.failed = None;
        }
        if self.fidelity.is_none() {
            self.revision = Some(revision);
            return Ok(());
        }
        let f = self.fidelity.as_mut().ok_or("fidelity allocation")?;
        if resized {
            f.resize_unpublished(renderer.gpu_mut(), settings.render_size, size)?;
            if let Some(effects) = &mut self.effects {
                effects.resize(renderer.gpu_mut(), settings.render_size)?;
            }
        }
        settings.configure(renderer.gpu_mut(), f)?;
        f.prepare_transport(renderer.gpu_mut())?;
        self.plan = Some(settings);
        for n in 0..settle.max(1) {
            let residency = f.refine_native_textures(renderer.gpu_mut())?;
            f.render(
                renderer.gpu_mut(),
                &FrameInput {
                    camera: &editor.state.camera.spec(),
                    tick: self.tick,
                    delta_seconds: 1. / 144.,
                    reset: replaced && n == 0,
                    lighting_changed: false,
                    deterministic: false,
                },
            )?;
            if let Some(effects) = &mut self.effects
                && !effects.is_empty()
            {
                effects.composite_ticks(
                    renderer.gpu_mut(),
                    f,
                    &editor.state.camera.spec(),
                    self.tick,
                    0,
                )?;
            }
            self.tick = self.tick.wrapping_add(1);
            self.remaining = self.remaining.saturating_sub(1);
            if residency.pending_textures > 0 {
                self.remaining = self.remaining.max(1);
            }
        }
        PresentationImage::publish(&mut self.published, renderer.gpu_mut(), f.output(), size)?;
        self.revision = Some(revision);
        Ok(())
    }
    pub fn needs_frame(&self, quality: bool) -> bool {
        self.frame_pending
            || self.content_job.is_some()
            || self.content_ready.is_some()
            || (quality || self.raster.is_some())
                && self.active
                && self.lit
                && ((self.fidelity.is_some() || self.raster.is_some()) && self.remaining > 0
                    || self.preview_needs_frame())
    }
    pub fn attach(
        &self,
        gpu: &mut NativeUi,
        renderer: &Renderer,
        quality: bool,
    ) -> ara_ui::Result<()> {
        if renderer.presentation_ready() {
            super::attach(gpu, renderer)?;
        } else {
            gpu.forget_external(overlay::viewport_image());
            gpu.forget_external(overlay::ENTITY_IMAGE);
        }
        if (quality || self.raster.is_some())
            && (self.lit || self.raster.is_some())
            && self.active
            && !(self.raster.is_some() && renderer.scene_frame_async())
            && let Some(image) = &self.published
        {
            // SAFETY: This view and renderer share the live device; all submissions complete and restore CopySource.
            unsafe {
                gpu.external_image(
                    overlay::viewport_image(),
                    renderer.gpu().native_image(image.output())?,
                    renderer.gpu().image_view(image.output())?,
                )?;
            }
        }
        Ok(())
    }
    /// Captures the same completed color the Scene viewport displays; the raster
    /// capture supplies picking/mark metadata only. Render this view first.
    pub(super) fn capture(
        &self,
        renderer: &mut Renderer,
        request: &CaptureRequest,
    ) -> anyhow::Result<ara_render::capture::Capture> {
        renderer.wait_scene_frame()?;
        let mut capture = if renderer.is_native() {
            renderer.capture_completed(request)?
        } else {
            renderer.capture(request)?
        };
        if self.active && matches!(request.mode, ViewMode::Lit) && !renderer.scene_frame_async() {
            self.published
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Scene has no completed fidelity frame"))?
                .capture(renderer, &mut capture)?;
        }
        Ok(capture)
    }
    pub fn destroy(self, renderer: &mut Renderer) -> ara_ui::Result<()> {
        renderer.wait_scene_frame()?;
        if let Some(candidate) = self.content_ready {
            candidate.scene.raster.destroy(renderer)?;
        }
        if let Some(raster) = self.raster {
            raster.destroy(renderer)?;
        }
        if let Some(image) = self.published {
            image.destroy(renderer.gpu_mut())?;
        }
        if let Some(effects) = self.effects {
            effects.destroy(renderer.gpu_mut())?;
        }
        if let Some(f) = self.fidelity {
            f.destroy(renderer.gpu_mut())?;
        }
        Ok(())
    }
}

/// Viewport View Mode names (the `editor.view` protocol) to renderer debug views.
fn view_mode(name: &str) -> ViewMode {
    match name {
        "baseColor" => ViewMode::Albedo,
        "normals" => ViewMode::Normals,
        "depth" => ViewMode::Depth,
        "roughnessMetal" => ViewMode::RoughnessMetal,
        "entityIds" => ViewMode::EntityIds,
        "shadowCascades" => ViewMode::ShadowCascades,
        _ => ViewMode::Lit,
    }
}
fn lit_fidelity(editor: &Editor) -> bool {
    editor.state.quality && matches!(view_mode(&editor.state.view_mode), ViewMode::Lit)
}

fn texture_config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1 << 30,
        upload_bytes: 16 << 20,
    }
}

fn pose_key(gesture: Option<&crate::world::gesture::Gesture>) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    gesture.map(|gesture| {
        let mut hash = std::hash::DefaultHasher::new();
        for (id, matrix) in &gesture.current {
            id.hash(&mut hash);
            for word in matrix.to_cols_array() {
                word.to_bits().hash(&mut hash);
            }
        }
        hash.finish()
    })
}

#[cfg(test)]
mod patch_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use ara_ui_render::gpu::UiGpu;

    fn submit_ui(scene: &SceneView, renderer: &Renderer, ui: &mut NativeUi) -> anyhow::Result<()> {
        scene
            .attach(ui, renderer, true)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ui.begin(renderer.size())
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ui.end().map_err(|error| anyhow::anyhow!("{error}"))?;
        renderer.gpu().assert_clean()?;
        Ok(())
    }

    #[test]
    fn scene_captures_native_quality_environment_and_effects_after_resize() -> anyhow::Result<()> {
        for ray_tracing in [false, true] {
            exercise_scene_view(ray_tracing)?;
        }
        Ok(())
    }

    fn exercise_scene_view(ray_tracing: bool) -> anyhow::Result<()> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/renderer-unification/scene-view")
            .join(format!("{}-{ray_tracing}", std::process::id()));
        std::fs::create_dir_all(root.join("levels"))?;
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Scene presentation test","main":"levels/main.scene","rendering":{"quality":"high","rayTracing":ray_tracing}}),
            ),
        )?;
        std::fs::write(
            root.join("levels/main.scene"),
            ara_json::to_canonical_string(&json!({
                "$schema":"ara://schema/scene/1","id":"sc_nativeview","records":{
                    "e_box":{"name":"Box","Transform":{},"MeshRenderer":{"color":[0.15,0.5,0.2,1]}},
                    "e_pending":{"name":"Pending model","Transform":{"position":[4,0,0]},"MeshRenderer":{"model":"@pending.gltf#as_pending"}},
                    "e_sky":{"SkyAtmosphere":{},"PostProcess":{"exposure":"manual","ev100":12}},
                "e_sun":{"Transform":{"rotation":[-45,0,0]},"Light":{"kind":"directional","intensity":24000}},
                    "e_fire":{"Transform":{"position":[1,0,0]},"VisualEffect":{"effect":"campfire","scale":0.3,"intensity":0.2}}
                }
            })),
        )?;
        // The Scene opens before its asset owner exists, just like a cold editor.
        let mut editor = Editor::open(&root)?;
        let view = editor.view()?;
        let create = if ray_tracing {
            Renderer::new
        } else {
            Renderer::new_raster
        };
        let mut renderer = create(
            view.scene(editor.host.root(), &editor.state, None, &mut editor.models)?,
            [192, 108],
            &[],
            root.join("gpu"),
        )?;
        let mut scene = SceneView::default();
        let result = (|| -> anyhow::Result<()> {
            // SAFETY: The renderer outlives this adapter, which is dropped before
            // leaving this closure; one thread serializes all queue submissions.
            let mut ui = unsafe { NativeUi::new(renderer.gpu().context())? };
            assert!(!renderer.presentation_ready());
            submit_ui(&scene, &renderer, &mut ui)?;
            scene
                .render(&mut renderer, &editor, &view, 2)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            if !ray_tracing {
                submit_ui(&scene, &renderer, &mut ui)?;
                scene
                    .render_capture(&mut renderer, &editor, &view, 2)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
            assert!(scene.active);
            assert!(renderer.presentation_ready());
            submit_ui(&scene, &renderer, &mut ui)?;
            if !ray_tracing {
                let allocation = scene
                    .raster_frame
                    .as_ref()
                    .expect("native frame")
                    .draws
                    .as_ptr();
                editor.extraction_revision += 1;
                scene.invalidate();
                scene
                    .render(&mut renderer, &editor, &view, 1)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                assert!(
                    scene.content_job.is_none(),
                    "already shown publication must not queue extraction"
                );
                assert_eq!(
                    scene
                        .raster_frame
                        .as_ref()
                        .expect("retained frame")
                        .draws
                        .as_ptr(),
                    allocation
                );
                // Hold a real CPU candidate unfinished across all typed material,
                // light, rename and drag patches. Live calls must neither join it
                // nor replace it; explicit capture later drains the superseded job.
                let key = content::Key {
                    revision: scene.revision.expect("prepared Scene revision"),
                    epoch: scene.content_epoch,
                    pose: None,
                    scene: content::scene_key(&editor),
                };
                let job = content::Job::start(
                    key,
                    scene.content.as_ref().expect("Scene cache"),
                    &editor,
                    &view,
                )?;
                let patched_allocation = scene
                    .raster_frame
                    .as_ref()
                    .expect("shown frame")
                    .draws
                    .as_ptr();
                let (release, wait) = std::sync::mpsc::channel();
                scene.content_job = Some(content::Job {
                    key,
                    thread: std::thread::spawn(move || {
                        wait.recv()?;
                        job.thread
                            .join()
                            .map_err(|_| anyhow::anyhow!("test content worker"))?
                    }),
                });
                scene.invalidate();
                super::patch_tests::exercise(&mut scene, &mut renderer, &mut editor, &view, &root)?;
                assert!(
                    !scene
                        .content_job
                        .as_ref()
                        .expect("same pending job")
                        .thread
                        .is_finished()
                );
                release.send(())?;
                // An earlier transient upload failure must not suppress an
                // explicit capture's retry/drain of the requested generation.
                scene.failed = scene.revision;
                scene
                    .render_capture(&mut renderer, &editor, &view, 2)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                assert!(scene.content_job.is_none());
                assert!(scene.failed.is_none());
                assert_eq!(
                    scene
                        .raster_frame
                        .as_ref()
                        .expect("shown patched frame")
                        .draws
                        .as_ptr(),
                    patched_allocation,
                    "stale worker must not rebuild already-patched input"
                );
                scene.failed = scene.revision;
                let mut preview = GamePreview::default();
                let camera = editor.state.camera.spec();
                let output = scene
                    .preview_game(
                        &mut renderer,
                        &editor,
                        &view,
                        &mut preview,
                        PreviewCamera {
                            camera: &camera,
                            size: [96, 54],
                            quality: None,
                        },
                    )
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                assert!(
                    output.is_none(),
                    "the first preview does not wait for its GPU frame"
                );
                assert!(preview.pending(&renderer));
                renderer.wait_scene_frame()?;
                assert!(
                    preview
                        .poll(&mut renderer)
                        .map_err(|error| anyhow::anyhow!("{error}"))?
                        .is_some(),
                    "explicit preview retries the failed generation"
                );
                let shown_frame = scene
                    .raster_frame
                    .as_ref()
                    .expect("shown frame")
                    .draws
                    .as_ptr();
                let shown_revision = scene.revision;
                for _ in 0..16 {
                    scene
                        .preview_game(
                            &mut renderer,
                            &editor,
                            &view,
                            &mut preview,
                            PreviewCamera {
                                camera: &camera,
                                size: [96, 54],
                                quality: None,
                            },
                        )
                        .map_err(|error| anyhow::anyhow!("{error}"))?;
                    renderer.wait_scene_frame()?;
                    assert!(
                        scene.content_job.is_none(),
                        "unchanged preview must reuse published content"
                    );
                    assert_eq!(scene.revision, shown_revision);
                    assert_eq!(
                        scene
                            .raster_frame
                            .as_ref()
                            .expect("shown frame")
                            .draws
                            .as_ptr(),
                        shown_frame
                    );
                }
                assert!(scene.failed.is_none());
                preview
                    .release(&mut renderer)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
            assert_eq!(scene.content.as_ref().expect("Scene content").revision(), 0);
            // The empty publication lands later. Neither no snapshot nor an
            // unpublished model may attach undefined images or kill the Scene.
            let _assets = ara_play_host::assets::Assets::new(&root, &root.join("unused-worker"))
                .map_err(anyhow::Error::msg)?;
            scene
                .render_capture(&mut renderer, &editor, &view, 1)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            assert!(scene.content.as_ref().expect("Scene content").revision() > 0);
            if ray_tracing {
                let fidelity = scene.fidelity.as_ref().expect("opt-in RT Scene view");
                assert_eq!(fidelity.settings().render_size, [96, 54]);
                assert_eq!(fidelity.settings().post.ev100, Some(12.));
                assert!(!scene.effects.as_ref().expect("Scene effects").is_empty());
            } else {
                assert!(renderer.is_native());
                assert!(scene.raster.is_some());
                assert!(scene.fidelity.is_none());
                assert!(!renderer.gpu().context().info.ray_tracing);
                assert_eq!(
                    renderer.native_geometry_stats().expect("native pool").blas,
                    0
                );
            }
            let request = CaptureRequest {
                camera: editor.state.camera.spec(),
                size: renderer.size(),
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 1,
                tick: 0,
                seed: 7,
                marks: None,
            };
            let capture = scene.capture(&mut renderer, &request)?;
            capture.write(&root.join("scene-high.png"))?;
            assert!(capture.image.pixels().any(|pixel| pixel[0] != pixel[1]));
            scene
                .render_capture(&mut renderer, &editor, &view, 8)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let first = scene.capture(&mut renderer, &request)?;
            scene
                .render(&mut renderer, &editor, &view, 5)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            scene
                .render_capture(&mut renderer, &editor, &view, 8)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let repeated = scene.capture(&mut renderer, &request)?;
            first.write(&root.join("first-capture.png"))?;
            repeated.write(&root.join("repeated-capture.png"))?;
            let difference = first
                .image
                .as_raw()
                .iter()
                .zip(repeated.image.as_raw())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            assert!(
                first.image == repeated.image,
                "capture must reset history on the live view without another descriptor set (RT={ray_tracing}, maximum channel difference {difference})"
            );
            renderer.resize([240, 136])?;
            assert_eq!(renderer.presentation_ready(), renderer.scene_frame_async());
            // Retain the published Scene color while dropping stale working IDs.
            submit_ui(&scene, &renderer, &mut ui)?;
            scene
                .render(&mut renderer, &editor, &view, 2)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            renderer.wait_scene_frame()?;
            assert_eq!(
                if ray_tracing {
                    scene.published.as_ref().expect("completed image").size()
                } else {
                    renderer.output_size()
                },
                [240, 136]
            );
            // Interactive resizes retain the published frame while releasing the
            // old fidelity targets before allocating their replacements.
            for size in [[192, 108], [256, 144], [240, 136]] {
                renderer.resize(size)?;
                submit_ui(&scene, &renderer, &mut ui)?;
                scene
                    .render(&mut renderer, &editor, &view, 1)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                renderer.wait_scene_frame()?;
                assert_eq!(
                    if ray_tracing {
                        scene.published.as_ref().expect("resized frame").size()
                    } else {
                        renderer.output_size()
                    },
                    size
                );
                if ray_tracing {
                    assert_eq!(
                        scene
                            .fidelity
                            .as_ref()
                            .expect("live view")
                            .settings()
                            .display_size,
                        size
                    );
                }
            }
            // Opening another document reuses this same view; it must not acquire
            // a second set of storage-image descriptors beside the old one.
            let created = editor.apply(
                vec![json!({"cmd":"doc.create","args":{
                    "path":"levels/reopened.scene","kind":"scene"
                }})],
                "Create reopened scene fixture",
                None,
            )?;
            let doc = created["outputs"][0]["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("created scene"))?;
            editor.apply_to(doc, vec![json!({"cmd":"entity.create","args":{
                "name":"Reopened box","components":{"Transform":{},"MeshRenderer":{"color":[0.3,0.6,0.2,1]}}
            }})], "Populate reopened scene fixture")?;
            for path in ["levels/reopened.scene", "levels/main.scene"] {
                editor.action("editor.scene.open", json!({"scene":path}))?;
                let reopened = editor.view()?;
                renderer.replace_scene(reopened.scene(
                    editor.host.root(),
                    &editor.state,
                    None,
                    &mut editor.models,
                )?)?;
                scene
                    .render_capture(&mut renderer, &editor, &reopened, 1)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                assert!(scene.active);
                assert!(
                    scene.failed.is_none(),
                    "scene reopen must publish without heap exhaustion"
                );
            }
            renderer.gpu().assert_clean()?;
            Ok(())
        })();
        scene
            .destroy(&mut renderer)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        result
    }
}
