//! Pilot Selected Camera (`view.camera.pilot`): the Scene view takes a perspective
//! Camera's pose, navigation moves it transiently, and each completed move commits as
//! one guarded undoable edit. Editing the Camera or an ancestor elsewhere stops it.
mod model;
use crate::Editor;
use crate::state::Camera;
use serde_json::{Value, json};
use std::sync::Arc;

/// The open scene's entities, resolved (the Camera and its ancestors are checked in it).
pub(crate) type Lookup = Arc<ara_doc::prefab::Resolved>;
struct Pending {
    base: u64,
    before: Camera,
}
struct Session {
    guard: model::Guard,
    checked: u64,
    baseline: Camera,
    observed: Camera,
    pending: Option<Pending>,
}
/// One explicit pilot target; it belongs to the shared Scene across native surfaces.
#[derive(Default)]
pub(crate) struct Pilot {
    session: Option<Session>,
    last: Value,
    note: Option<String>,
}

impl std::fmt::Debug for Pilot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pilot({})", self.report())
    }
}

/// Projection changes use a separate navigation op before the full pose restore,
/// because Camera::set_projection intentionally adjusts the navigation radius.
pub(crate) fn restore(editor: &mut Editor, view: Camera) -> ara_ui::Result<()> {
    if editor.state.camera.projection != view.projection {
        editor.action("editor.camera", json!({"projection":view.projection}))?;
    }
    editor.action("editor.camera",json!({"target":view.target.to_array(),"yaw":view.yaw,"pitch":view.pitch,"distance":view.distance,"fovYDegrees":view.fov_y_degrees,"orthographicHeight":view.orthographic_height}))?;
    Ok(())
}
impl Pilot {
    pub(crate) fn toggle(
        &mut self,
        editor: &mut Editor,
        entity: Option<&str>,
        lookup: &mut impl FnMut(&Editor) -> ara_ui::Result<Lookup>,
    ) -> ara_ui::Result<()> {
        if self.session.is_some() {
            self.cancel(editor)?;
            let session = self.session.take().ok_or("Pilot session disappeared")?;
            self.note = Some(format!("Stopped piloting {}", session.guard.name));
            self.last = json!({"outcome":"unpiloted","entity":session.guard.entity});
            return Ok(());
        }
        let entity = entity.ok_or("Select a Camera to pilot")?;
        model::view_rotation(&editor.state.camera)?;
        let resolved = lookup(editor)?;
        let guard = model::Guard::capture(editor, &resolved, entity)?;
        let expected = guard.aligned_view(editor.state.camera)?;
        editor.action("editor.camera", json!({"entity":entity}))?;
        let aligned = editor.state.camera;
        if !aligned.is_valid()
            || aligned.position().distance(expected.position()) > 0.001
            || model::view_rotation(&aligned)?
                .dot(model::view_rotation(&expected)?)
                .abs()
                < 0.999999
            || (aligned.fov_y_degrees - expected.fov_y_degrees).abs() > 0.0001
        {
            return Err("The Scene view did not align with the selected Camera".into());
        }
        self.note = Some(format!(
            "Piloting {} · Esc cancels a move · Pilot Camera again to stop",
            guard.name
        ));
        self.last = json!({"outcome":"piloting","entity":entity,"authoringChanged":false});
        self.session = Some(Session {
            checked: editor.host.store().revision(),
            guard,
            baseline: aligned,
            observed: aligned,
            pending: None,
        });
        Ok(())
    }
    pub(crate) fn cancel(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(session) = &mut self.session else {
            return Ok(false);
        };
        let Some(pending) = session.pending.take() else {
            return Ok(false);
        };
        if editor.state.doc == session.guard.doc {
            restore(editor, pending.before)?;
            session.baseline = pending.before;
            session.observed = pending.before;
        }
        self.last = json!({"outcome":"cancelled","entity":session.guard.entity,"base":pending.base,"authoringChanged":false});
        self.note = Some(format!(
            "Cancelled camera move · Piloting {}",
            session.guard.name
        ));
        Ok(true)
    }
    pub(crate) fn poll(
        &mut self,
        editor: &mut Editor,
        active: bool,
        native_change: bool,
        lookup: &mut impl FnMut(&Editor) -> ara_ui::Result<Lookup>,
    ) -> ara_ui::Result<bool> {
        let Some(mut session) = self.session.take() else {
            return Ok(false);
        };
        let before_revision = editor.host.store().revision();
        let result = self.drive(editor, &mut session, active, native_change, lookup);
        match result {
            Ok(changed) => {
                self.session = Some(session);
                Ok(changed)
            }
            Err(error) => {
                // A failed gesture never writes a compensating authoring edit.
                // Only restore its own transient view while this Scene still owns it.
                if let Some(pending) = session.pending.take()
                    && editor.state.doc == session.guard.doc
                    && (native_change || editor.state.camera == session.observed)
                {
                    restore(editor, pending.before)?;
                }
                let message = format!("Pilot stopped: {error}");
                self.note = Some(message.clone());
                self.last = json!({"outcome":"stopped","entity":session.guard.entity,"error":message,"authoringChanged":editor.host.store().revision()!=before_revision});
                Err(error)
            }
        }
    }
    fn drive(
        &mut self,
        editor: &mut Editor,
        session: &mut Session,
        active: bool,
        native_change: bool,
        lookup: &mut impl FnMut(&Editor) -> ara_ui::Result<Lookup>,
    ) -> ara_ui::Result<bool> {
        if editor.state.doc != session.guard.doc {
            return Err("the open Scene changed".into());
        }
        let revision = editor.host.store().revision();
        if revision != session.checked {
            session
                .guard
                .validate(editor, lookup(editor)?.as_ref(), session.checked)?;
            session.checked = revision;
        }
        let current = editor.state.camera;
        if current != session.observed && !native_change {
            return Err("the Scene view changed outside this native camera gesture".into());
        }
        if active || native_change {
            model::view_rotation(&current)?;
        }
        session.observed = current;
        if active {
            if session.pending.is_none() {
                session.pending = Some(Pending {
                    base: revision,
                    before: session.baseline,
                });
            }
            return Ok(false);
        }
        if session.pending.is_none() && !native_change {
            return Ok(false);
        }
        let pending = session.pending.take().unwrap_or(Pending {
            base: revision,
            before: session.baseline,
        });
        let commands = match session.guard.commands(&current) {
            Ok(commands) => commands,
            Err(error) => {
                session.pending = Some(pending);
                return Err(error);
            }
        };
        if commands.is_empty() {
            session.baseline = current;
            return Ok(false);
        }
        let applied = editor.apply(commands, "Pilot camera navigation", Some(pending.base));
        let receipt = match applied {
            Ok(receipt) => receipt,
            Err(error) => {
                session.pending = Some(pending);
                return Err(error.into());
            }
        };
        session.guard =
            model::Guard::capture(editor, lookup(editor)?.as_ref(), &session.guard.entity)?;
        session.checked = editor.host.store().revision();
        session.baseline = current;
        self.last = json!({"outcome":"committed","entity":session.guard.entity,"base":pending.base,"revision":session.checked,"receipt":receipt});
        self.note = Some(format!(
            "Camera move saved · Piloting {}",
            session.guard.name
        ));
        Ok(true)
    }
    pub(crate) fn report(&self) -> Value {
        json!({"active":self.session.is_some(),"entity":self.session.as_ref().map(|s|s.guard.entity.as_str()),"scene":self.session.as_ref().map(|s|s.guard.doc.as_str()),"pending":self.session.as_ref().is_some_and(|s|s.pending.is_some()),"base":self.session.as_ref().and_then(|s|s.pending.as_ref().map(|p|p.base)),"last":self.last})
    }
    /// Whether a Camera is being piloted.
    pub(crate) fn active(&self) -> bool {
        self.session.is_some()
    }
    /// The piloted Camera's entity.
    pub(crate) fn target(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.guard.entity.as_str())
    }
    /// What the last step has to say in the status bar, once.
    pub(crate) fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }
}
