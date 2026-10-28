//! Borrow retained native geometry with its current authoring and pose provenance.
use super::*;
use crate::shell::overlay::diagnostics::{Source, SourceKey};

impl SceneView {
    /// Never substitutes a pending extraction or reads GPU data for diagnostic bounds.
    pub(in crate::app) fn diagnostic_source(&self, editor: &Editor) -> Source<'_> {
        let scene = content::scene_key(editor);
        let shown = self.raster_input;
        let current = self.raster_scene.as_deref() == Some(editor.state.doc.as_str())
            && shown.is_some_and(|key| {
                key.revision.0 == editor.host.store().revision()
                    && key.revision.2 == editor.review_revision
                    && key.epoch == self.content_epoch
                    && key.pose == pose_key(editor.gesture.as_ref())
                    && key.scene == scene
                    && self
                        .content
                        .as_ref()
                        .is_some_and(|content| key.revision.3 == content.revision())
            });
        let native = self
            .raster
            .as_ref()
            .and_then(ara_runtime_render::RasterView::scene);
        Source {
            scene: native.or_else(|| self.prepared.as_ref().map(|prepared| &prepared.scene)),
            frame: native.and(self.raster_frame.as_ref()),
            key: SourceKey {
                document_revision: editor.host.store().revision(),
                content_epoch: shown.map_or(self.content_epoch, |key| key.epoch),
                publication_revision: shown.map_or(0, |key| key.revision.3),
                pose_revision: shown.and_then(|key| key.pose),
                scene,
            },
            current,
        }
    }
}
