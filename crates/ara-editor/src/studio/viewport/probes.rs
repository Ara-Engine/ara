//! Probe calls borrow the exact retained raster frame; no extraction, GPU wait or I/O.
use super::{SceneView, content, pose_key};
use crate::{Editor, probe_capture::SceneBinding};
use ara_diag::{Diagnostic, codes};
use ara_runtime_render::probes::{
    CancelOutcome, ProbeFilter, ProbeJob, ProbeJobStatus, ProbeKey, ProbeSnapshot, RecaptureRequest,
};
use std::path::Path;

impl SceneView {
    /// Only evaluated while a probe owner exists, before a path switch can drop it.
    pub(crate) fn probe_replacement_required(
        &self,
        renderer: &ara_render::Renderer,
        editor: &Editor,
    ) -> ara_ui::Result<bool> {
        if self.raster.is_none() {
            return Ok(false);
        }
        let settings = crate::world::gltf::render_settings(editor.host.store().workspace())?;
        let plan = super::Plan::resolve(
            editor
                .state
                .render_quality
                .unwrap_or(settings.quality.into()),
            &super::Capabilities::of(renderer.gpu()),
            renderer.size(),
            ara_runtime_render::RenderOptions {
                ray_tracing: settings.ray_tracing,
            },
        )?;
        Ok(!matches!(plan.path, super::RenderPath::Raster(_)))
    }
    fn validate_probe_frame(
        &self,
        editor: &Editor,
        doc: &str,
        revision: u64,
    ) -> Result<(), Vec<Diagnostic>> {
        let fail = |message| {
            vec![
                Diagnostic::error(codes::PROBE_MISSING, message)
                    .doc(doc)
                    .path("ReflectionProbe"),
            ]
        };
        if editor.state.doc != doc || editor.host.store().revision() != revision {
            return Err(fail(
                "The requested probe Scene/source is no longer current",
            ));
        }
        if editor.gesture.is_some() || self.preview_view.is_some() {
            return Err(fail(
                "Finish or cancel the current authoring preview before capturing probes",
            ));
        }
        if self.raster_scene.as_deref() != Some(doc)
            || self.raster_frame.is_none()
            || self
                .raster
                .as_ref()
                .is_none_or(|raster| !raster.has_prepared_content())
        {
            return Err(fail(
                "The current Scene has no matching retained native raster frame yet",
            ));
        }
        let (Some(input), Some(content)) = (self.raster_input, self.content.as_ref()) else {
            return Err(fail(
                "The retained Scene has no validated content provenance yet",
            ));
        };
        let current = content::Key {
            revision: (
                revision,
                editor.extraction_revision,
                editor.review_revision,
                content.revision(),
            ),
            epoch: self.content_epoch,
            pose: pose_key(editor.gesture.as_ref()),
            scene: content::scene_key(editor),
        };
        if !input.same_input(current) || self.raster_snapshot_dirty || self.failed.is_some() {
            return Err(fail(
                "The current authored Scene has not reached its retained raster frame yet",
            ));
        }
        Ok(())
    }

    /// Cheap provenance check used only while captures are active or a read is due.
    pub(crate) fn probe_source_matches(
        &self,
        editor: &Editor,
        binding: &SceneBinding,
    ) -> bool {
        self.validate_probe_frame(editor, &binding.doc, binding.source_revision)
            .is_ok()
            && self.raster.as_ref().is_some_and(|raster| {
                raster
                    .probes()
                    .snapshot(&ProbeFilter::default(), Some(u64::MAX))
                    .scene_revision
                    == binding.scene_revision
            })
    }

    pub(crate) fn probe_has_binding(&self, cache_root: &Path) -> bool {
        self.raster
            .as_ref()
            .is_some_and(|raster| raster.probes().cache_root() == cache_root)
    }

    /// Call only after the owner has retired jobs for the previous document.
    pub(crate) fn bind_probe_scene(
        &mut self,
        editor: &Editor,
        cache_root: &Path,
    ) -> Result<(), Vec<Diagnostic>> {
        self.validate_probe_frame(editor, &editor.state.doc, editor.host.store().revision())?;
        let raster = self
            .raster
            .as_mut()
            .expect("validated retained raster frame");
        raster
            .probes_mut()
            .set_scene(&editor.state.doc, cache_root.to_path_buf());
        Ok(())
    }

    pub(crate) fn recapture_probes(
        &mut self,
        editor: &Editor,
        doc: &str,
        source_revision: u64,
        entities: &[String],
        why: Option<&str>,
    ) -> Result<(SceneBinding, ProbeJob), Vec<Diagnostic>> {
        self.validate_probe_frame(editor, doc, source_revision)?;
        let raster = self
            .raster
            .as_mut()
            .expect("validated retained raster frame");
        let frame = self
            .raster_frame
            .as_ref()
            .expect("validated retained raster frame");
        let scene_revision = raster
            .probes()
            .snapshot(&ProbeFilter::default(), Some(u64::MAX))
            .scene_revision;
        let probes: Vec<_> = entities
            .iter()
            .map(|entity| ProbeKey {
                doc: doc.to_owned(),
                entity: entity.clone(),
            })
            .collect();
        let job = raster.probes_mut().recapture(
            frame,
            &RecaptureRequest {
                probes: &probes,
                why,
            },
        )?;
        Ok((
            SceneBinding {
                doc: doc.to_owned(),
                scene_revision,
                source_revision,
            },
            job,
        ))
    }

    /// Full listing; the Host cache applies caller filters and since locally.
    pub(crate) fn probe_snapshot(
        &self,
        editor: &Editor,
    ) -> Result<(SceneBinding, ProbeSnapshot), Vec<Diagnostic>> {
        let source_revision = editor.host.store().revision();
        self.validate_probe_frame(editor, &editor.state.doc, source_revision)?;
        let snapshot = self
            .raster
            .as_ref()
            .expect("validated retained raster frame")
            .probes()
            .snapshot(&ProbeFilter::default(), None);
        Ok((
            SceneBinding {
                doc: editor.state.doc.clone(),
                scene_revision: snapshot.scene_revision,
                source_revision,
            },
            snapshot,
        ))
    }

    /// These intentionally remain usable for the old Scene while it is retiring.
    pub(crate) fn probe_job(&self, job: ProbeJob) -> Option<ProbeJobStatus> {
        self.raster.as_ref()?.probes().job(job)
    }
    pub(crate) fn cancel_probe_job(&mut self, job: ProbeJob) -> Option<CancelOutcome> {
        self.raster.as_mut()?.probes_mut().cancel(job)
    }
    pub(crate) fn wake_probe_work(&mut self) {
        self.remaining = self.remaining.max(2);
        self.convergence.reset();
    }

    /// Retain this exact raster owner until its bake/hash/cache work has retired.
    /// No submission, fence wait or filesystem work runs on this owner tick.
    pub(crate) fn probe_retirement_pending(
        &mut self,
        renderer: &mut ara_render::Renderer,
    ) -> Result<bool, Diagnostic> {
        let raster = self.raster.as_mut().ok_or_else(|| {
            Diagnostic::error(
                codes::PROBE_MISSING,
                "The retiring probe raster owner is missing; retirement cannot be confirmed",
            )
        })?;
        raster.poll_probe_retirement(renderer).map_err(|error| {
            Diagnostic::error(
                crate::codes::RENDER,
                format!("Probe retirement polling failed: {error}"),
            )
        })
    }

    /// One explicit native-verification observation of the completed Scene image.
    /// Request metadata does not establish the mode: inspect the retained raster
    /// presentation and current source before reading its already completed output.
    pub(crate) fn completed_debug_view_observation(
        &self,
        renderer: &mut ara_render::Renderer,
        editor: &Editor,
        requested: &str,
        scene_ready: bool,
    ) -> ara_ui::Result<Option<serde_json::Value>> {
        use ara_render::capture::{CaptureRequest, ViewMode};
        let expected = match requested {
            "gi_diffuse" => ViewMode::GiDiffuse,
            "overdraw" => ViewMode::Overdraw,
            _ => return Err("Unknown native debug-view observation".into()),
        };
        if !scene_ready
            || self.frame_pending
            || renderer.scene_frame_pending()
            || !renderer.presentation_ready()
            || !renderer.is_native()
            || self.raster_presentation.as_ref().map(|(mode, _)| *mode) != Some(expected)
            || self
                .validate_probe_frame(editor, &editor.state.doc, editor.host.store().revision())
                .is_err()
        {
            return Ok(None);
        }
        let request = CaptureRequest {
            camera: editor.state.camera.spec(),
            size: renderer.size(),
            mode: expected,
            deterministic: true,
            exposure: self
                .raster_frame
                .as_ref()
                .map_or(1.0, |frame| frame.environment.exposure),
            settle_frames: 1,
            tick: 0,
            seed: 7,
            marks: None,
        };
        // The caller requests this only once per measured mode. This reads the
        // completed live image; it never asks capture() to render another frame.
        let captured = renderer.capture_completed(&request)?;
        Ok(Some(serde_json::json!({
            "mode":expected.name(), "completed":true, "current":true,
            "completedModeSource":"SceneView.raster_presentation",
            "doc":editor.state.doc, "sourceRevision":editor.host.store().revision(),
            "sceneHash":captured.metadata.scene_hash,
            "size":[captured.image.width(),captured.image.height()],
            "overdraw":captured.metadata.overdraw,
            "readbackScope":"one requested completed native image; no re-render"
        })))
    }
}

#[cfg(test)]
mod completed_debug_tests {
    #[test]
    fn debug_mode_names_cannot_fall_back_to_lit() {
        assert_eq!(
            super::super::view_mode("gi_diffuse"),
            ara_render::capture::ViewMode::GiDiffuse
        );
        assert_eq!(
            super::super::view_mode("overdraw"),
            ara_render::capture::ViewMode::Overdraw
        );
    }
}
