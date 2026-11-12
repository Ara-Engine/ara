//! Keeps the Scene view in step with the document. Any change invalidates it and the content
//! worker extracts the frame again (`viewport::scene_frame`): there is no in-place patching of
//! a renderer's scene. Typed Inspector drafts are projected on the document
//! (`SceneView::set_preview_view`); a gizmo drag changes the pose the frame is keyed by.
use super::viewport::SceneView;
use crate::Editor;
use crate::world::DocumentView;
use std::collections::BTreeMap;

/// What a window previews in the Scene without committing: its Inspector drafts (compared
/// to skip unchanged frames) and the records they make.
pub(crate) trait Drafts {
    /// The drafts, keyed by what they edit; equal maps preview the same records.
    fn drafts(&self) -> &BTreeMap<String, String>;
    /// The selection's records with the drafts applied, or None when nothing previews.
    fn draft_records(
        &self,
        editor: &Editor,
        view: &DocumentView,
    ) -> Option<BTreeMap<String, ara_doc::Record>>;
}

/// The Scene view's state relative to the committed view: which previews it shows.
pub(crate) struct SceneSync {
    /// The drafts the typed preview reflects (None: build it again).
    drafts: Option<BTreeMap<String, String>>,
    /// The gizmo pose the drag preview reflects.
    pose: Option<BTreeMap<String, glam::Mat4>>,
    /// The Scene view shows a projected draft (`SceneView::set_preview_view`).
    projected: bool,
}

/// How a Scene update went (the responsiveness probe reports it).
#[derive(Default)]
pub(crate) struct Stages {
    /// Whether the Scene view took the change by extracting again.
    pub native: bool,
}

impl SceneSync {
    pub fn new() -> Self {
        SceneSync {
            drafts: None,
            pose: None,
            projected: false,
        }
    }

    /// The document changed: the Scene view extracts its frame again.
    pub fn commit(&mut self, scene_view: &mut SceneView) -> Stages {
        self.forget();
        scene_view.invalidate();
        Stages { native: true }
    }

    /// Show this frame's preview on top of `view`: the gizmo drag (its pose keys the frame),
    /// else the typed Inspector drafts projected, else nothing. Returns whether the Scene
    /// changed.
    pub fn preview(
        &mut self,
        view: &DocumentView,
        editor: &Editor,
        shell: &dyn Drafts,
        scene_view: &mut SceneView,
    ) -> bool {
        if let Some(gesture) = &editor.gesture {
            if self.pose.as_ref() == Some(&gesture.current) {
                return false;
            }
            self.pose = Some(gesture.current.clone());
            return true;
        }
        let dragged = self.pose.take().is_some();
        if !dragged && self.drafts.as_ref() == Some(shell.drafts()) {
            return false;
        }
        self.drafts = Some(shell.drafts().clone());
        let target = shell
            .draft_records(editor, view)
            .map(|records| view.with_records(records));
        if target.is_none() && !self.projected {
            return dragged;
        }
        self.projected = target.is_some();
        scene_view.set_preview_view(&editor.state.doc, target);
        true
    }

    fn forget(&mut self) {
        self.drafts = None;
        self.pose = None;
        self.projected = false;
    }
}
