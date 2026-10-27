//! Complete explicit Frame requests when immutable imported bounds arrive, through the same typed action.
use super::*;
use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    sync::Arc,
};

pub(crate) struct PendingFrame {
    doc: String,
    selection: Vec<String>,
    camera: crate::state::Camera,
    opening: bool,
    all: bool,
    aspect: f32,
    agent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_bounds_never_override_a_new_human_camera_or_selection() {
        let mut state = EditorState {
            doc: "sc_test001".into(),
            selection: vec!["e_box001".into()],
            ..EditorState::default()
        };
        let frame = PendingFrame::new(&state, false, false, 1., false);
        assert!(frame.still_requested(&state, "human"));
        state.camera.target.x += 1.;
        assert!(!frame.still_requested(&state, "human"));
        state.camera = frame.camera;
        state.selection.clear();
        assert!(!frame.still_requested(&state, "human"));
    }
    #[test]
    fn a_pending_agent_frame_observes_only_that_agents_presence() {
        let mut state = EditorState {
            doc: "sc_agent01".into(),
            selection: vec!["e_agent01".into()],
            ..EditorState::default()
        };
        let frame = PendingFrame::new(&state, false, false, 1., true);
        state.presence.insert(
            "agent".into(),
            crate::state::AgentPresence {
                author: "Agent".into(),
                doc: state.doc.clone(),
                selection: state.selection.clone(),
                camera: state.camera,
            },
        );
        state.doc = "sc_human01".into();
        state.selection.clear();
        state.camera.target.x += 10.;
        assert!(frame.still_requested(&state, "agent"));
        state.presence.remove("agent");
        assert!(!frame.still_requested(&state, "agent"));
    }
}
impl PendingFrame {
    pub(super) fn new(
        state: &EditorState,
        opening: bool,
        all: bool,
        aspect: f32,
        agent: bool,
    ) -> Self {
        Self {
            doc: state.doc.clone(),
            selection: state.selection.clone(),
            camera: state.camera,
            opening,
            all,
            aspect,
            agent,
        }
    }
    fn still_requested(&self, state: &EditorState, session: &str) -> bool {
        if self.agent {
            state.presence.get(session).is_some_and(|presence| {
                presence.doc == self.doc
                    && presence.selection == self.selection
                    && presence.camera == self.camera
            })
        } else {
            state.doc == self.doc
                && state.selection == self.selection
                && state.camera == self.camera
        }
    }
}
impl EditorService {
    /// Only the document/publication set is hashed per tick. Resolution and metadata
    /// requests occur at those boundaries; ordinary camera input stays free of asset work.
    pub(super) fn poll_bounds(&mut self, cx: &mut Cx<'_>) {
        let mut hash = std::hash::DefaultHasher::new();
        self.navigation.state.doc.hash(&mut hash);
        for presence in self.navigation.state.presence.values() {
            presence.doc.hash(&mut hash);
        }
        for frame in self.pending_frames.values() {
            frame.doc.hash(&mut hash);
        }
        let key = (cx.store().revision(), self.bounds.revision(), hash.finish());
        if self.bounds_key != Some(key) {
            let docs: BTreeSet<_> = std::iter::once(self.navigation.state.doc.clone())
                .chain(
                    self.navigation
                        .state
                        .presence
                        .values()
                        .map(|presence| presence.doc.clone()),
                )
                .chain(self.pending_frames.values().map(|frame| frame.doc.clone()))
                .collect();
            let mut resolved = Vec::new();
            for doc in docs {
                if let Ok(entities) = self.resolver.resolve(cx.store(), &doc) {
                    resolved.push(entities);
                }
            }
            self.bounds
                .set_documents(resolved.iter().map(Arc::as_ref), cx.store().workspace());
            self.bounds_key = Some(key);
        }
        let changed = self.bounds.poll();
        for error in self.bounds.take_failures() {
            cx.log(
                Diagnostic::warning(crate::codes::ACTION, error)
                    .subject(ara_diag::Subject::system("editor")),
            );
        }
        if !changed {
            return;
        }
        let sessions: Vec<_> = self.pending_frames.keys().cloned().collect();
        for session in sessions {
            let Some(frame) = self.pending_frames.get(&session) else {
                continue;
            };
            if !frame.still_requested(&self.navigation.state, &session) {
                self.pending_frames.remove(&session);
                continue;
            }
            let Ok(entities) = self.resolver.resolve(cx.store(), &frame.doc) else {
                self.pending_frames.remove(&session);
                continue;
            };
            let selection = if frame.all || frame.opening {
                &[][..]
            } else {
                &frame.selection
            };
            if !self.bounds.settled(&entities, selection) {
                continue;
            }
            let Some(frame) = self.pending_frames.remove(&session) else {
                continue;
            };
            let params = json!({"session":session,"opening":frame.opening,"all":frame.all,"aspect":frame.aspect});
            // A closed caller cancels its request. A live caller retains attribution;
            // any command failure is reported through the ordinary diagnostics bus.
            if let Ok(Err(diagnostics)) =
                cx.resume_session(&session, |cx| self.dispatch(cx, "editor.frame", params))
            {
                for diagnostic in diagnostics {
                    cx.log(diagnostic);
                }
            }
        }
    }
}
