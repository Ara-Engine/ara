//! View ownership for asynchronous navigation hints, independent of authoring revisions.
use crate::state::{AgentPresence, Camera, EditorState};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Shared only during editor/shell construction; input events update one scalar.
#[derive(Clone, Default)]
pub(crate) struct Interaction(Arc<AtomicU64>);

impl Interaction {
    /// Call before human pointer-down, key-down, text or wheel ingress. Releases
    /// and callback completions do not take ownership of the human's view.
    pub fn note(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub fn revision(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn capture(&self, state: &EditorState, session: &str, agent: bool) -> NavigationOwner {
        NavigationOwner(if agent {
            Owner::Agent(state.presence.get(session).cloned())
        } else {
            Owner::Human {
                revision: self.revision(),
                context: HumanContext::capture(state),
            }
        })
    }
}

pub(crate) struct NavigationOwner(Owner);

enum Owner {
    Human {
        revision: u64,
        context: HumanContext,
    },
    Agent(Option<AgentPresence>),
}

#[derive(PartialEq)]
struct HumanContext {
    doc: String,
    selection: Vec<String>,
    focus: String,
    focus_field: Option<String>,
    resource: Option<String>,
    camera: Camera,
    asset_path: Option<String>,
    asset_folder: String,
    canvas: crate::graph::Camera,
    canvas_selection: std::collections::BTreeSet<String>,
}

impl HumanContext {
    fn capture(state: &EditorState) -> Self {
        Self {
            doc: state.doc.clone(),
            selection: state.selection.clone(),
            focus: state.focus.clone(),
            focus_field: state.focus_field.clone(),
            resource: state.resource.clone(),
            camera: state.camera,
            asset_path: state.asset_path.clone(),
            asset_folder: state.asset_folder.clone(),
            canvas: state.canvas,
            canvas_selection: state.canvas_selection.clone(),
        }
    }

    fn matches(&self, state: &EditorState) -> bool {
        self.doc == state.doc
            && self.selection == state.selection
            && self.focus == state.focus
            && self.focus_field == state.focus_field
            && self.resource == state.resource
            && self.camera == state.camera
            && self.asset_path == state.asset_path
            && self.asset_folder == state.asset_folder
            && self.canvas == state.canvas
            && self.canvas_selection == state.canvas_selection
    }
}

impl NavigationOwner {
    /// Only the hint is conditional: callers still apply authoring to the
    /// captured document under the ordinary Store revision validation.
    pub fn suppression(
        &self,
        interaction: &Interaction,
        state: &EditorState,
        session: &str,
    ) -> Option<&'static str> {
        match &self.0 {
            Owner::Human { revision, .. } if *revision != interaction.revision() => {
                Some("human input continued while the C# editor callback ran")
            }
            Owner::Human { context, .. } if !context.matches(state) => {
                Some("human navigation changed while the C# editor callback ran")
            }
            Owner::Agent(presence) if presence.as_ref() != state.presence.get(session) => Some(
                "the invoking agent's private navigation changed while the C# editor callback ran",
            ),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
