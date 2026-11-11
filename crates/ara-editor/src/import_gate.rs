//! What the studio's import gate and the protocol share. The gate itself lives in the
//! window (`studio::loading`); this holds the import report both read from the published
//! import feed, and the two flags they trade: the window says whether the gate holds, an
//! agent asks for it to open (`editor.imports`).
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ara_doc::Workspace;
use ara_play_host::ImportProgress;
use serde_json::{Value, json};

/// Where a project's imports stand, by project path ([`ImportProgress`] by asset id).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Imports {
    /// Sources with a current import.
    pub done: usize,
    /// Registered sources.
    pub total: usize,
    /// Sources still importing.
    pub pending: usize,
    /// Failed sources: project path and the import's error, sorted by path.
    pub failed: Vec<(String, String)>,
}

impl Imports {
    /// Every import settled, whether or not some failed.
    pub(crate) fn settled(&self) -> bool {
        self.pending == 0
    }
}

/// Asset id → source path, from the import documents' paths.
pub(crate) fn source_paths(workspace: &Workspace) -> BTreeMap<String, String> {
    workspace
        .docs()
        .filter(|(_, d)| d.kind == "import")
        .map(|(path, d)| {
            let source = path.strip_suffix(".import.json").unwrap_or(path);
            (d.id.to_string(), source.to_owned())
        })
        .collect()
}

/// `progress` by project path.
pub(crate) fn imports(progress: &ImportProgress, paths: &BTreeMap<String, String>) -> Imports {
    let path = |id: &str| paths.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let mut failed: Vec<(String, String)> = progress
        .failed
        .iter()
        .map(|(id, error)| (path(id), error.clone()))
        .collect();
    failed.sort();
    Imports {
        done: progress.done,
        total: progress.total,
        pending: progress.pending.len(),
        failed,
    }
}

/// The report `editor.imports` answers: the stage, the counts and every failed import with
/// its error. `imports` is `None` until the pipeline first reports.
pub(crate) fn report(imports: Option<&Imports>, holding: bool) -> Value {
    let (stage, done, total, pending, failed) = match imports {
        None => ("preparing", 0, 0, 0, Vec::new()),
        Some(i) => (
            match (i.pending, i.failed.is_empty()) {
                (0, true) => "settled",
                (0, false) => "review",
                _ => "importing",
            },
            i.done,
            i.total,
            i.pending,
            i.failed
                .iter()
                .map(|(path, error)| json!({"path": path, "error": error}))
                .collect(),
        ),
    };
    json!({
        "gate": if holding { "holding" } else { "open" },
        "stage": stage,
        "done": done,
        "total": total,
        "pending": pending,
        "failed": failed,
    })
}

#[derive(Default)]
struct State {
    holding: bool,
    release: bool,
}

/// The gate's state shared between the window and the protocol service.
#[derive(Clone, Default)]
pub(crate) struct Shared(Arc<Mutex<State>>);

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// The window says whether the gate holds its views.
    pub(crate) fn set_holding(&self, holding: bool) {
        self.state().holding = holding;
    }
    /// Whether the window's gate holds its views (false with no studio window).
    pub(crate) fn holding(&self) -> bool {
        self.state().holding
    }
    /// An agent asks the gate to open.
    pub(crate) fn request_release(&self) {
        self.state().release = true;
    }
    /// Whether an agent asked the gate to open since the last call.
    pub(crate) fn take_release(&self) -> bool {
        std::mem::take(&mut self.state().release)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_names_each_stage_and_lists_failures_with_their_errors() {
        assert_eq!(report(None, true)["stage"], "preparing");
        let importing = Imports {
            done: 1,
            total: 3,
            pending: 2,
            failed: Vec::new(),
        };
        assert_eq!(report(Some(&importing), true)["stage"], "importing");
        let review = Imports {
            done: 2,
            total: 3,
            pending: 0,
            failed: vec![("models/a.gltf".into(), "ARA4302 missing buffer".into())],
        };
        let said = report(Some(&review), false);
        assert_eq!(said["stage"], "review");
        assert_eq!(said["gate"], "open");
        assert_eq!(said["failed"][0]["path"], "models/a.gltf");
        assert_eq!(said["failed"][0]["error"], "ARA4302 missing buffer");
        let settled = Imports {
            done: 3,
            total: 3,
            ..Imports::default()
        };
        assert_eq!(report(Some(&settled), false)["stage"], "settled");
    }

    #[test]
    fn a_release_request_is_seen_once() {
        let shared = Shared::default();
        assert!(!shared.take_release());
        shared.request_release();
        assert!(shared.take_release());
        assert!(!shared.take_release());
        shared.set_holding(true);
        assert!(shared.holding());
    }
}
