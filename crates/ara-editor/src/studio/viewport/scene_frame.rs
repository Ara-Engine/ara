//! The Scene view's frame. The editor extracts it itself (`world::gltf::extract_frame`) on the
//! content worker; the worker hands it straight back with the CPU scene picking, snapping and
//! bounds read, and this view adopts it and has Renderlab draw it. No renderer other than
//! Renderlab exists: `ara_render` is not involved, and `SceneDevice` is only the GPU device
//! Renderlab's image lives on.
use super::*;
use crate::world::gltf::FidelityContent;

/// How many passes Renderlab draws after a change before the Scene rests.
const SETTLE: u32 = 24;

/// What the Renderlab image on screen was drawn from: the Scene's content revision, the
/// camera, the size and the Mesh Paint revision. The next pass draws again only when it differs.
pub(super) type Drawn = (renderlab::Revision, crate::state::Camera, [u32; 2], u64);

impl SceneView {
    /// One pass of the Scene view: keeps the extracted frame current (a worker extracts it when
    /// the document, review, content, pose or drafts change), adopts a finished one, and has
    /// Renderlab draw it when the frame, the camera or the size changed.
    pub(super) fn render_scene_frame(
        &mut self,
        device: &mut SceneDevice,
        editor: &Editor,
        view: &crate::world::DocumentView,
    ) -> ara_ui::Result<()> {
        ara_gpu::cpu_scope!("SceneView.render_scene_frame");
        if self.preview_view.is_some() && self.preview_scene != editor.state.doc {
            // Drafts belong to the outgoing scene.
            self.set_preview_view(&editor.state.doc, None);
        }
        let store = editor.host.store();
        let content = match &mut self.content {
            Some(content) => content,
            slot @ None => slot.insert(FidelityContent::new(editor.host.root())?),
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
            && self.frame.is_some()
            && self.shown_input.is_some_and(|shown| shown.same_input(key))
        {
            // A settled model-cache notification after this view's own adoption: nothing to
            // extract again.
            self.revision = Some(revision);
            self.patched_pose = key.pose;
        }
        let changed = self.revision != Some(revision)
            || self.shown_input.is_none_or(|shown| !shown.same_input(key))
            || (editor.gesture.is_some() && self.patched_pose != pose_key(editor.gesture.as_ref()))
            || self.frame.is_none();
        content::poll(
            &mut self.content_job,
            &mut self.content_ready,
            key,
            self.shown_input,
            false,
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
        let mut adopted = false;
        if let Some(candidate) = self.content_ready.take() {
            let epoch = candidate.key.epoch;
            let prepared = candidate.scene;
            *content = prepared.content;
            // Arrivals only mattered to the raster path's fade-in.
            drop(content.take_arrivals());
            self.frame = Some(prepared.frame);
            self.prepared = Some(prepared.scene);
            self.scene_doc = Some(editor.state.doc.clone());
            self.revision = Some((
                revision.0,
                revision.1,
                revision.2,
                content.snapshot_revision(),
            ));
            self.shown_input = self.revision.map(|revision| content::Key {
                revision,
                epoch,
                ..key
            });
            self.patched_pose = key.pose;
            adopted = true;
        }
        // A Mesh Paint stroke's colours stream into the drawn instances; when it ends, the
        // document (committed or not) extracts again.
        if let Some(frame) = self.frame.as_mut()
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
        // Models arriving or failing change their placeholders' look in place, without a new
        // extraction.
        if let Some(frame) = self.frame.as_mut()
            && let Some(true) =
                content.style_pending(frame, self.preview_view.as_ref().unwrap_or(view))
        {
            self.scene_settle = SETTLE;
        }
        if self.frame.is_none() {
            return Ok(());
        }
        let drawn: Drawn = (
            self.revision,
            editor.state.camera,
            device.size(),
            editor.paint_revision,
        );
        if adopted || self.scene_drawn != Some(drawn) {
            self.scene_drawn = Some(drawn);
            self.scene_settle = SETTLE;
        }
        // Renderlab refines its image over the frames after a change (its pipelines warm, its
        // acceleration structures build, its history converges): draw a few, then rest. A frame
        // is submitted without waiting for it (one in flight, the newest camera each time) and
        // taken by the pass that finds it done, so the studio keeps running at its own rate.
        self.poll_renderlab(device);
        if self.scene_settle > 0 && !self.renderlab.as_ref().is_some_and(|r| r.in_flight()) {
            self.scene_settle -= 1;
            self.start_renderlab(device, editor);
            // A frame that finished inside `start` (an engine that cannot split one) is the still
            // the capture routes read.
            self.publish_still(device)?;
        }
        self.active = self.presentable();
        Ok(())
    }

    /// Submits the Scene's frame to the editor-hosted Renderlab engine from the editor's camera;
    /// [`Self::poll_renderlab`] takes its image. A failure (the engine did not start, or could not
    /// draw) is kept in `renderlab_error` for the Scene panel to show.
    fn start_renderlab(&mut self, device: &mut SceneDevice, editor: &Editor) {
        if self.renderlab_error.is_some() {
            return;
        }
        let Some(content) = self.content.as_mut() else {
            return;
        };
        let store = editor.host.store();
        let camera = crate::world::gltf::camera_view(&editor.state.camera.spec());
        if let (Some(frame), Some(camera)) = (self.frame.as_mut(), camera) {
            frame.camera = Some(camera);
        }
        let drawn = (|| -> anyhow::Result<()> {
            if self.renderlab.is_none() {
                self.renderlab = Some(renderlab::RenderlabScene::new(
                    &editor.host.root().join(".ara/gpu"),
                )?);
            }
            let frame = self
                .frame
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no Scene frame"))?;
            let look = ara_runtime_render::environment_at(frame, editor.state.camera.position());
            let size = device.size();
            self.renderlab
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("no Renderlab engine"))?
                .start(
                    device.gpu_mut(),
                    frame,
                    &look,
                    size,
                    self.revision,
                    editor.state.camera,
                    || content.content(frame, store.workspace(), store.schema()),
                )
        })();
        if let Err(error) = drawn {
            self.fail_renderlab(device, &error);
        }
    }

    /// Takes the frame Renderlab finished, if it has one: never waits. A frame of another size
    /// than the Scene view's now (a resize since it was submitted) is not shown.
    fn poll_renderlab(&mut self, device: &mut SceneDevice) {
        let Some(scene) = self.renderlab.as_mut() else {
            return;
        };
        match scene.poll(device.gpu_mut()) {
            Ok(true) => {
                super::super::trace::log(|| "scene frame taken".into());
                if let Err(error) = self.publish_still(device) {
                    self.fail_renderlab(device, &error);
                }
            }
            Ok(false) => {}
            Err(error) => self.fail_renderlab(device, &error),
        }
    }

    /// The still the capture routes read: Renderlab's image, copied when it changes.
    fn publish_still(&mut self, device: &mut SceneDevice) -> ara_ui::Result<()> {
        if let Some(output) = self.renderlab.as_ref().and_then(|scene| scene.image())
            && self.published_from != Some(output)
        {
            self.published_from = Some(output);
            let size = device.size();
            PresentationImage::publish(&mut self.published, device.gpu_mut(), output, size)?;
        }
        Ok(())
    }

    /// Reports a Renderlab failure once and releases the engine: a failed engine (a lost device,
    /// say) shows nothing, so the Scene is the error, not a stale or black frame.
    fn fail_renderlab(&mut self, device: &mut SceneDevice, error: &dyn std::fmt::Display) {
        eprintln!("Renderlab could not draw the Scene view: {error:#}");
        self.renderlab_error = Some(format!("{error:#}"));
        if let Some(scene) = self.renderlab.take() {
            let _ = scene.release(device.gpu_mut());
        }
    }
}
