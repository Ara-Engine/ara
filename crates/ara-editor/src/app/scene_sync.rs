//! Keeps the Scene renderer in step with the document. Value edits, typed Inspector
//! drafts and gizmo drags patch it in place ([`Patch`]: moved meshes alone, the
//! light list, materials, labels); anything else extracts and uploads the whole
//! scene, as does a new model publication (placeholders become models).
use super::*;
use crate::world::{DocumentView, Patch};
use std::collections::BTreeMap;

/// How long a newer model publication must hold still before a whole re-upload
/// adopts it: imports publish in bursts, and each upload of a large scene costs
/// most of a second on this thread.
const PUBLICATION_SETTLE: Duration = Duration::from_secs(1);

/// The renderer's state relative to the committed view: which previews it shows,
/// and which model publication its geometry came from.
pub(super) struct SceneSync {
    /// `ModelCache::revision` behind the last whole upload.
    models: u64,
    /// A newer publication, and since when it has held still.
    publication: Option<(u64, Instant)>,
    /// Meshes and lights a preview (typed draft or gizmo drag) moved away from the
    /// committed view; the next patch also restores them.
    shown: Patch,
    /// The drafts the typed preview reflects (None: build it again).
    drafts: Option<BTreeMap<String, String>>,
    /// The gizmo pose the drag preview reflects.
    pose: Option<BTreeMap<String, glam::Mat4>>,
}

/// How long a Scene update took, by stage (the responsiveness probe reports them).
#[derive(Default)]
pub(super) struct Stages {
    /// Whole-scene extraction, in milliseconds.
    pub extract_ms: f64,
    /// The whole scene's upload, or a patch: extracting and uploading what moved.
    pub upload_ms: f64,
    /// Whether the change patched the renderer instead of replacing its geometry.
    pub patched: bool,
}

impl SceneSync {
    /// The renderer holds a scene extracted at model publication `models` (read before
    /// that extraction).
    pub fn new(models: u64) -> Self {
        SceneSync {
            models,
            publication: None,
            shown: Patch::default(),
            drafts: None,
            pose: None,
        }
    }

    /// Upload `view` whole once a newer model publication than the last whole
    /// upload's has held still for `PUBLICATION_SETTLE` while the user is `idle`
    /// (placeholders become models); returns whether it did.
    pub fn refresh(
        &mut self,
        view: &DocumentView,
        editor: &mut Editor,
        renderer: &mut Renderer,
        idle: bool,
    ) -> ara_ui::Result<bool> {
        let revision = editor.models.revision();
        if revision == self.models {
            self.publication = None;
            return Ok(false);
        }
        match self.publication {
            Some((seen, since))
                if seen == revision && idle && since.elapsed() >= PUBLICATION_SETTLE =>
            {
                self.rebuild(view, editor, renderer)?;
                Ok(true)
            }
            Some((seen, _)) if seen == revision => Ok(false),
            _ => {
                self.publication = Some((revision, Instant::now()));
                Ok(false)
            }
        }
    }

    /// The document changed from `old` to `new`: patch the renderer when the change
    /// allows it, else upload `new` whole. Previews are rebuilt on top afterwards.
    pub fn commit(
        &mut self,
        old: &DocumentView,
        new: &DocumentView,
        editor: &mut Editor,
        renderer: &mut Renderer,
    ) -> ara_ui::Result<Stages> {
        if let Some(patch) = old.patch_to(new, &editor.state.hidden) {
            let patch = patch.union(&self.shown);
            let at = Instant::now();
            match new.apply(
                &patch,
                renderer,
                editor.host.root(),
                &editor.state,
                None,
                &mut editor.models,
            ) {
                Ok(()) => {
                    self.forget();
                    return Ok(Stages {
                        upload_ms: at.elapsed().as_secs_f64() * 1000.,
                        patched: true,
                        ..Default::default()
                    });
                }
                Err(error) => eprintln!("Scene patch failed, uploading the scene: {error}"),
            }
        }
        self.rebuild(new, editor, renderer)
    }

    /// Upload `view` whole, the gizmo drag included; typed previews are rebuilt next.
    pub fn rebuild(
        &mut self,
        view: &DocumentView,
        editor: &mut Editor,
        renderer: &mut Renderer,
    ) -> ara_ui::Result<Stages> {
        // Read first: a publication adopted during extraction triggers another.
        self.models = editor.models.revision();
        self.publication = None;
        let at = Instant::now();
        let scene = view.scene(
            editor.host.root(),
            &editor.state,
            editor.gesture.as_ref(),
            &mut editor.models,
        )?;
        let extract_ms = at.elapsed().as_secs_f64() * 1000.;
        let at = Instant::now();
        renderer.update_geometry(scene)?;
        self.forget();
        if let Some(gesture) = &editor.gesture {
            self.shown = view.gesture_patch(gesture, &editor.state.hidden);
            self.pose = Some(gesture.current.clone());
        }
        Ok(Stages {
            extract_ms,
            upload_ms: at.elapsed().as_secs_f64() * 1000.,
            patched: false,
        })
    }

    /// Show this frame's preview on top of `view`: the gizmo drag, else the typed
    /// Inspector drafts, else nothing (restoring what a finished preview moved).
    /// Returns whether the Scene changed.
    pub fn preview(
        &mut self,
        view: &DocumentView,
        editor: &mut Editor,
        shell: &Shell,
        renderer: &mut Renderer,
    ) -> ara_ui::Result<bool> {
        let hidden = &editor.state.hidden;
        let (target, patch, gesture) = if let Some(gesture) = &editor.gesture {
            if self.pose.as_ref() == Some(&gesture.current) {
                return Ok(false);
            }
            self.pose = Some(gesture.current.clone());
            self.drafts = None;
            (None, view.gesture_patch(gesture, hidden), Some(gesture))
        } else {
            self.pose = None;
            if self.drafts.as_ref() == Some(shell.drafts()) {
                return Ok(false);
            }
            self.drafts = Some(shell.drafts().clone());
            // A draft only previews when a patch can show it (a reshaped mesh waits
            // for its commit).
            let target = shell
                .draft_records(editor, view)
                .map(|records| view.with_records(records))
                .and_then(|target| Some((view.patch_to(&target, hidden)?, target)));
            match target {
                Some((patch, target)) => (Some(target), patch, None),
                None => (None, Patch::default(), None),
            }
        };
        let apply = patch.clone().union(&self.shown);
        if apply.is_empty() {
            return Ok(false);
        }
        let result = target.as_ref().unwrap_or(view).apply(
            &apply,
            renderer,
            editor.host.root(),
            &editor.state,
            gesture,
            &mut editor.models,
        );
        match result {
            Ok(()) => self.shown = patch,
            Err(error) => {
                eprintln!("Scene preview failed, uploading the scene: {error}");
                let drafts = self.drafts.take();
                self.rebuild(view, editor, renderer)?;
                // Retry when the drafts change, not every frame.
                self.drafts = drafts;
            }
        }
        Ok(true)
    }

    fn forget(&mut self) {
        self.shown = Patch::default();
        self.drafts = None;
        self.pose = None;
    }
}
