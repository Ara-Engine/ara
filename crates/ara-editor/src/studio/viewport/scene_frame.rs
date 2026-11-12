//! The Scene view's frame without the old renderer. With `ARA_SCENE_RENDERER=renderlab` the
//! frame Renderlab draws is the one the editor extracts itself (`world::gltf::extract_frame`) on
//! the content worker; the worker hands it straight back with the CPU scene picking, snapping
//! and bounds read, and this view adopts it. No `RasterView` and no `ara_render` raster content
//! is built, published or prepared on this path (`ara_render::Renderer` is only the device that
//! carries Renderlab's image to the compositor).
//!
//! `ARA_SCENE_RASTER_FRAME=1` keeps the old raster frame beside Renderlab (an A/B for the
//! comparison tests).
//!
//! The old raster path keeps its own job (`render_raster_attempt`); a capture, or Renderlab
//! failing, returns to it through [`SceneView::leave_scene_frame`].
use super::*;

/// How many passes Renderlab draws after a change before the Scene rests.
const SETTLE: u32 = 24;

/// What the Renderlab image on screen was drawn from: the Scene's content revision, the
/// camera, the size and the Mesh Paint revision. The next pass draws again only when it differs.
pub(super) type Drawn = (renderlab::Revision, crate::state::Camera, [u32; 2], u64);

impl SceneView {
    /// One pass of the Renderlab Scene view: keeps the extracted frame current (a worker
    /// extracts it when the document, review, content, pose or drafts change), adopts a finished
    /// one, and has Renderlab draw it when the frame, the camera or the size changed.
    pub(super) fn render_scene_frame(
        &mut self,
        renderer: &mut Renderer,
        editor: &Editor,
        view: &crate::world::DocumentView,
    ) -> ara_ui::Result<()> {
        ara_gpu::cpu_scope!("SceneView.render_scene_frame");
        self.enter_scene_frame(renderer)?;
        if self.preview_view.is_some() && self.preview_scene != editor.state.doc {
            // Drafts belong to the outgoing scene.
            self.set_preview_view(&editor.state.doc, None);
        }
        let store = editor.host.store();
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
        if self.revision != Some(revision)
            && self.raster_frame.is_some()
            && self.raster_input.is_some_and(|shown| shown.same_input(key))
        {
            // A settled model-cache notification after this view's own adoption: nothing to
            // extract again.
            self.revision = Some(revision);
            self.patched_pose = key.pose;
        }
        let changed = self.revision != Some(revision)
            || self.raster_input.is_none_or(|shown| !shown.same_input(key))
            || (editor.gesture.is_some() && self.patched_pose != pose_key(editor.gesture.as_ref()))
            || self.raster_frame.is_none();
        content::poll(
            &mut self.content_job,
            &mut self.content_ready,
            key,
            self.raster_input,
            renderer,
            false,
        )?;
        if changed && self.content_job.is_none() && self.content_ready.is_none() {
            content::queue_reason(self.revision, key);
            self.content_job = Some(content::Job::start_frame(
                key,
                content,
                editor,
                self.preview_view.as_ref().unwrap_or(view),
            )?);
        }
        let mut adopted = false;
        if self
            .content_ready
            .as_ref()
            .is_some_and(|candidate| candidate.ready)
        {
            let candidate = self
                .content_ready
                .take()
                .ok_or("Scene content publication")?;
            let epoch = candidate.key.epoch;
            let prepared = candidate.scene;
            let (Some(frame), Some(scene)) = (prepared.frame, prepared.scene) else {
                // Raster content from an earlier path: extract again.
                self.revision = None;
                return Ok(());
            };
            *content = prepared.content;
            // Arrivals only fade models in the raster path.
            drop(content.take_arrivals());
            self.raster_frame = Some(frame);
            self.prepared = Some(scene);
            self.fidelity_scene = Some(editor.state.doc.clone());
            self.revision = Some((
                revision.0,
                revision.1,
                revision.2,
                content.snapshot_revision(),
            ));
            self.raster_input = self.revision.map(|revision| content::Key {
                revision,
                epoch,
                ..key
            });
            self.patched_pose = key.pose;
            self.failed = None;
            adopted = true;
        }
        // A Mesh Paint stroke's colours stream into the drawn instances; when it ends, the
        // document (committed or not) extracts again.
        if let Some(frame) = self.raster_frame.as_mut()
            && editor.paint_revision != self.paint_revision
        {
            self.paint_revision = editor.paint_revision;
            if let Some(preview) = &editor.paint_preview {
                for draw in &mut frame.draws {
                    if let Some(colors) = preview.get(&draw.entity) {
                        draw.paint = Some(std::sync::Arc::clone(colors));
                    }
                }
                self.paint_applied = true;
            } else if std::mem::take(&mut self.paint_applied) {
                self.revision = None;
            }
        }
        if self.raster_frame.is_none() {
            return Ok(());
        }
        let drawn: Drawn = (
            self.revision,
            editor.state.camera,
            renderer.size(),
            editor.paint_revision,
        );
        if adopted || self.scene_drawn != Some(drawn) {
            self.scene_drawn = Some(drawn);
            self.scene_settle = SETTLE;
        }
        // Renderlab refines its image over the passes after a change (its pipelines warm, its
        // acceleration structures build, its history converges): draw a few, then rest.
        if self.scene_settle > 0 {
            self.scene_settle -= 1;
            self.draw_renderlab(renderer, editor);
            // The still the capture routes read: Renderlab's image, as the raster still does.
            if let Some(output) = self.renderlab.as_ref().and_then(|scene| scene.image()) {
                let size = renderer.size();
                PresentationImage::publish(&mut self.published, renderer.gpu_mut(), output, size)?;
            }
        }
        self.frame_pending = false;
        self.active = self
            .renderlab
            .as_ref()
            .is_some_and(|scene| scene.image().is_some());
        Ok(())
    }

    /// Switches to the Scene view's own frame: the old raster view, its frame and its jobs are
    /// released (a capture, or the first pass).
    fn enter_scene_frame(&mut self, renderer: &mut Renderer) -> ara_ui::Result<()> {
        if self.scene_only {
            return Ok(());
        }
        self.scene_only = true;
        self.content_job = None;
        if let Some(candidate) = self.content_ready.take()
            && let Some(raster) = candidate.scene.raster
        {
            raster.destroy(renderer)?;
        }
        if let Some(raster) = self.raster.take() {
            raster.destroy(renderer)?;
            renderer.clear_native()?;
            self.raster_gpu_pending = false;
            self.raster_presentation = None;
            self.convergence.failed();
        }
        self.raster_frame = None;
        self.raster_input = None;
        self.raster_scene = None;
        self.scene_drawn = None;
        self.invalidate();
        Ok(())
    }

    /// Back to the old raster path (a capture, or Renderlab failed): what the Scene view's own
    /// frame left behind is dropped, so the raster path builds its own.
    pub(super) fn leave_scene_frame(&mut self) {
        if !self.scene_only {
            return;
        }
        self.scene_only = false;
        self.content_job = None;
        self.content_ready = None;
        self.raster_frame = None;
        self.raster_input = None;
        self.prepared = None;
        self.fidelity_scene = None;
        self.scene_drawn = None;
        self.invalidate();
    }
}
