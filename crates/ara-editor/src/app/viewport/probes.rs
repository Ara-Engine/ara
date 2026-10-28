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
    pub(in crate::app) fn probe_replacement_required(
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
    pub(in crate::app) fn probe_source_matches(
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

    pub(in crate::app) fn probe_has_binding(&self, cache_root: &Path) -> bool {
        self.raster
            .as_ref()
            .is_some_and(|raster| raster.probes().cache_root() == cache_root)
    }

    /// Call only after the owner has retired jobs for the previous document.
    pub(in crate::app) fn bind_probe_scene(
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

    pub(in crate::app) fn recapture_probes(
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
    pub(in crate::app) fn probe_snapshot(
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
    pub(in crate::app) fn probe_job(&self, job: ProbeJob) -> Option<ProbeJobStatus> {
        self.raster.as_ref()?.probes().job(job)
    }
    pub(in crate::app) fn cancel_probe_job(&mut self, job: ProbeJob) -> Option<CancelOutcome> {
        self.raster.as_mut()?.probes_mut().cancel(job)
    }
    pub(in crate::app) fn wake_probe_work(&mut self) {
        self.remaining = self.remaining.max(2);
        self.convergence.reset();
    }

    /// True means PENDING, matching VQ's forthcoming poll_probe_retirement API.
    /// The compile-only backend cannot create a job. Unexpected active work must
    /// remain owned until that real retirement seam lands; absence is not proof.
    pub(in crate::app) fn probe_retirement_pending(
        &mut self,
        _renderer: &mut ara_render::Renderer,
    ) -> Result<bool, Diagnostic> {
        Err(Diagnostic::error(
            codes::PROBE_UNSUPPORTED,
            "This renderer checkpoint has no probe-job retirement polling API; active ownership is retained",
        ))
    }
}
