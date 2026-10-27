//! Keeps the Scene renderer in step with the document. Value edits, typed Inspector
//! drafts and gizmo drags patch it in place ([`Patch`]: moved meshes alone, the
//! light list, materials, labels); anything else extracts and uploads the whole
//! scene, as does a new model publication (placeholders become models). A failed
//! upload is reported and retried; it never closes the editor. A native Scene
//! ([`native`]) takes the same patches through `SceneView::apply_patch`; a change no
//! patch covers invalidates it (it extracts its own instanced content again).
use super::viewport::SceneView;
use super::*;
use crate::world::{DocumentView, Patch};
use std::collections::BTreeMap;

/// Whether the Scene renders natively (`Renderer::is_native`), or will from its next
/// frame: the render plan takes the raster path, as it does for every project without
/// `rayTracing` (the plan `SceneView::render` resolves). Before that first frame the
/// renderer holds no native state, and a legacy upload would fail on any texture or
/// environment change.
pub(super) fn native(editor: &Editor, renderer: &Renderer) -> bool {
    renderer.is_native() || raster_plan(editor, renderer).unwrap_or(false)
}
fn raster_plan(editor: &Editor, renderer: &Renderer) -> ara_ui::Result<bool> {
    use ara_render_fidelity::presets::{Capabilities, Plan, RenderPath};
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
    Ok(matches!(plan.path, RenderPath::Raster(_)))
}

/// How long a newer model publication must hold still before the ray-traced Scene's
/// whole re-upload adopts it: imports publish in bursts, and each upload of a large
/// scene costs most of a second on this thread. A native Scene follows publications
/// itself.
const PUBLICATION_SETTLE: Duration = Duration::from_secs(1);
/// How long after a failed whole upload the next attempt waits (while idle).
const STALE_RETRY: Duration = Duration::from_secs(5);

/// The renderer's state relative to the committed view: which previews it shows,
/// and which model publication its geometry came from.
pub(super) struct SceneSync {
    /// `ModelCache::revision` behind the last whole upload.
    models: u64,
    /// A newer publication, and since when it has held still.
    publication: Option<(u64, Instant)>,
    /// When a whole upload last failed: the renderer still shows an older scene, so
    /// patches and previews wait until `refresh` uploads again.
    stale: Option<Instant>,
    /// Meshes and lights a preview (typed draft or gizmo drag) moved away from the
    /// committed view; the next patch also restores them.
    shown: Patch,
    /// The drafts the typed preview reflects (None: build it again).
    drafts: Option<BTreeMap<String, String>>,
    /// The gizmo pose the drag preview reflects.
    pose: Option<BTreeMap<String, glam::Mat4>>,
    /// The native Scene view shows a projected draft (`SceneView::set_preview_view`).
    projected: bool,
    /// The store revision the Scene reflects: documents changed after it that a
    /// view does not see (`world::scene_only`) need a full update.
    revision: u64,
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
    /// Whether a native renderer took the change (invalidated, nothing uploaded here).
    pub native: bool,
}

impl SceneSync {
    /// The renderer holds a scene extracted at model publication `models` (read before
    /// that extraction) and store `revision`.
    pub fn new(models: u64, revision: u64) -> Self {
        SceneSync {
            models,
            publication: None,
            stale: None,
            shown: Patch::default(),
            drafts: None,
            pose: None,
            projected: false,
            revision,
        }
    }

    /// Adopt a newer model publication (placeholders become models): a native Scene
    /// picks it up itself, so it is only recorded; the ray-traced Scene uploads `view`
    /// whole once the publication has held still for `PUBLICATION_SETTLE` and the user
    /// is `idle`, as does a failed upload due a retry. Returns whether it uploaded.
    pub fn refresh(
        &mut self,
        view: &DocumentView,
        editor: &mut Editor,
        renderer: &mut Renderer,
        scene_view: &mut SceneView,
        idle: bool,
    ) -> bool {
        if let Some(failed) = self.stale {
            if idle && failed.elapsed() >= STALE_RETRY {
                self.rebuild(view, editor, renderer, scene_view);
                return true;
            }
            return false;
        }
        let revision = editor.models.revision();
        if revision == self.models {
            self.publication = None;
            return false;
        }
        // A native Scene follows publications itself (its content key carries the
        // publication revision; snapshots and GPU preparation run on workers) and
        // renders them on its own schedule (after input, content work waits), so
        // this only records the adoption.
        if native(editor, renderer) {
            self.models = revision;
            self.publication = None;
            return false;
        }
        match self.publication {
            Some((seen, since))
                if seen == revision && idle && since.elapsed() >= PUBLICATION_SETTLE =>
            {
                self.rebuild(view, editor, renderer, scene_view);
                true
            }
            Some((seen, _)) if seen == revision => false,
            _ => {
                self.publication = Some((revision, Instant::now()));
                false
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
        scene_view: &mut SceneView,
    ) -> Stages {
        // A material, the render settings or another asset document is not in the
        // views: when one changed, only a full update shows it.
        let scene_only =
            crate::world::scene_only(editor.host.store(), self.revision, &editor.state.doc);
        if !scene_only {
            return self.rebuild(new, editor, renderer, scene_view);
        }
        if native(editor, renderer) {
            return self.commit_native(old, new, editor, renderer, scene_view);
        }
        self.revision = editor.host.store().revision();
        if self.stale.is_none()
            && let Some(patch) = old.patch_to(new, &editor.state.hidden)
        {
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
                    return Stages {
                        upload_ms: at.elapsed().as_secs_f64() * 1000.,
                        patched: true,
                        ..Default::default()
                    };
                }
                Err(error) => eprintln!("Scene patch failed, uploading the scene: {error}"),
            }
        }
        self.rebuild(new, editor, renderer, scene_view)
    }

    /// [`commit`](Self::commit) on a native Scene: the patch (with what a preview
    /// still shows) updates it in place; anything else invalidates it.
    fn commit_native(
        &mut self,
        old: &DocumentView,
        new: &DocumentView,
        editor: &mut Editor,
        renderer: &mut Renderer,
        scene_view: &mut SceneView,
    ) -> Stages {
        if self.stale.is_none()
            && let Some(patch) = old.patch_to(new, &editor.state.hidden)
        {
            let patch = patch.union(&self.shown);
            let at = Instant::now();
            match scene_view.apply_patch(renderer, editor, new, None, &patch) {
                Ok(true) => {
                    self.revision = editor.host.store().revision();
                    self.forget();
                    return Stages {
                        upload_ms: at.elapsed().as_secs_f64() * 1000.,
                        patched: true,
                        native: true,
                        ..Default::default()
                    };
                }
                Ok(false) => {}
                Err(error) => eprintln!("Scene patch failed, extracting the scene: {error}"),
            }
        }
        self.rebuild(new, editor, renderer, scene_view)
    }

    /// Upload `view` whole, the gizmo drag included; typed previews are rebuilt next.
    /// A failure is reported (Problems) and leaves the last good scene on screen. A
    /// native renderer is only invalidated: it extracts its own content.
    pub fn rebuild(
        &mut self,
        view: &DocumentView,
        editor: &mut Editor,
        renderer: &mut Renderer,
        scene_view: &mut SceneView,
    ) -> Stages {
        self.revision = editor.host.store().revision();
        if native(editor, renderer) {
            self.models = editor.models.revision();
            self.publication = None;
            self.stale = None;
            self.forget();
            scene_view.invalidate();
            return Stages {
                native: true,
                ..Default::default()
            };
        }
        match self.upload(view, editor, renderer) {
            Ok(stages) => stages,
            Err(error) => {
                self.stale = Some(Instant::now());
                editor.report_failure(&*error);
                Stages::default()
            }
        }
    }

    fn upload(
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
        self.stale = None;
        self.forget();
        if let Some(gesture) = &editor.gesture {
            self.shown = view.gesture_patch(gesture, &editor.state.hidden);
            self.pose = Some(gesture.current.clone());
        }
        Ok(Stages {
            extract_ms,
            upload_ms: at.elapsed().as_secs_f64() * 1000.,
            ..Default::default()
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
        scene_view: &mut SceneView,
    ) -> bool {
        if native(editor, renderer) {
            return self.project(view, editor, shell, renderer, scene_view);
        }
        if self.stale.is_some() {
            return false;
        }
        let hidden = &editor.state.hidden;
        let (target, patch, gesture) = if let Some(gesture) = &editor.gesture {
            if self.pose.as_ref() == Some(&gesture.current) {
                return false;
            }
            self.pose = Some(gesture.current.clone());
            self.drafts = None;
            (None, view.gesture_patch(gesture, hidden), Some(gesture))
        } else {
            self.pose = None;
            if self.drafts.as_ref() == Some(shell.drafts()) {
                return false;
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
            return false;
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
                self.rebuild(view, editor, renderer, scene_view);
                // Retry when the drafts change, not every frame.
                self.drafts = drafts;
            }
        }
        true
    }

    /// The native preview, patched in place (`SceneView::apply_patch`): each new
    /// gizmo pose, or the typed drafts projected on `view`. What the last preview
    /// moved is restored with the next patch. When the Scene cannot patch, a drag
    /// re-extracts on the next render and drafts go through `set_preview_view`.
    fn project(
        &mut self,
        view: &DocumentView,
        editor: &Editor,
        shell: &Shell,
        renderer: &mut Renderer,
        scene_view: &mut SceneView,
    ) -> bool {
        let hidden = &editor.state.hidden;
        if let Some(gesture) = &editor.gesture {
            if self.pose.as_ref() == Some(&gesture.current) {
                return false;
            }
            self.pose = Some(gesture.current.clone());
            let moved = view.gesture_patch(gesture, hidden);
            self.patch_native(view, None, moved, editor, renderer, scene_view);
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
        if target.is_none() && !self.projected && self.shown.is_empty() {
            return dragged;
        }
        let projected = target.is_some();
        let shows = match &target {
            Some(target) => view.patch_to(target, hidden),
            None => Some(Patch::default()),
        };
        let target = match shows {
            Some(shows) => {
                if self.patch_native(view, target, shows, editor, renderer, scene_view) {
                    self.projected = projected;
                    return true;
                }
                // The patch took the projection; build it again for the fallback.
                shell
                    .draft_records(editor, view)
                    .map(|records| view.with_records(records))
            }
            None => target,
        };
        self.shown = Patch::default();
        self.projected = projected;
        scene_view.set_preview_view(&editor.state.doc, target);
        true
    }

    /// Patch the native Scene to show `preview` (None: `view`), where `shows` is what
    /// it changes from `view`; what the previous preview changed is restored too.
    /// False when the Scene could not patch in place.
    fn patch_native(
        &mut self,
        view: &DocumentView,
        preview: Option<DocumentView>,
        shows: Patch,
        editor: &Editor,
        renderer: &mut Renderer,
        scene_view: &mut SceneView,
    ) -> bool {
        if self.stale.is_some() {
            return false;
        }
        let patch = shows.clone().union(&self.shown);
        match scene_view.apply_patch(renderer, editor, view, preview, &patch) {
            Ok(true) => {
                self.shown = shows;
                true
            }
            Ok(false) => false,
            Err(error) => {
                eprintln!("Scene preview patch failed, extracting the scene: {error}");
                false
            }
        }
    }

    fn forget(&mut self) {
        self.shown = Patch::default();
        self.drafts = None;
        self.pose = None;
        self.projected = false;
    }
}
