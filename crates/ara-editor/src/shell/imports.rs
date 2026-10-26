//! Import progress from the play host's pipeline (`ara_play_host::ImportProgress`):
//! the status bar's Importing chip, Assets badges and the Scene's loading notice.
use super::commands::Target;
use super::*;
use ara_play_host::{ImportProgress, PublishedAssets};
use std::{collections::BTreeSet, path::Path, sync::Arc};

/// The read-only progress feed and the last progress read from it. Polling costs one
/// atomic load per frame; the progress is re-read only when the owner republishes.
pub(super) struct Imports {
    reader: Option<PublishedAssets>,
    /// The feed revision `progress` was read at (memoized panels key on it).
    pub revision: u64,
    /// The latest progress; `None` until the pipeline starts.
    pub progress: Option<Arc<ImportProgress>>,
    /// Asset IDs the open scene uses, keyed by store revision and scene.
    scene: Option<(u64, String, BTreeSet<String>)>,
}

/// Where one asset's import stands, when it is not simply done.
pub(super) enum Import<'a> {
    Pending,
    /// The latest import failed with this error.
    Failed(&'a str),
}

impl Imports {
    pub fn new(root: &Path) -> Self {
        Self {
            reader: PublishedAssets::for_project(root).ok(),
            revision: 0,
            progress: None,
            scene: None,
        }
    }
    /// Re-reads progress when the owner republished it; true when it changed.
    pub fn poll(&mut self) -> bool {
        let Some(reader) = &self.reader else {
            return false;
        };
        let revision = reader.progress_revision();
        if revision == self.revision {
            return false;
        }
        // The owner is mid-publication: keep the last progress and retry next frame.
        let Some(progress) = reader.progress() else {
            return false;
        };
        self.revision = revision;
        let changed = self.progress.as_deref() != Some(&*progress);
        self.progress = Some(progress);
        changed
    }
    /// The import state of a browser asset (by the ID in its reference).
    pub fn of(&self, asset: &assets::Asset) -> Option<Import<'_>> {
        let progress = self.progress.as_deref()?;
        let (_, id) = asset.reference.as_deref()?.rsplit_once('#')?;
        if progress.pending.contains(id) {
            Some(Import::Pending)
        } else {
            progress.failed.get(id).map(|error| Import::Failed(error))
        }
    }
    /// Asset IDs the open scene uses, through the prefabs and materials it references.
    fn scene_assets(&mut self, editor: &Editor) -> &BTreeSet<String> {
        let revision = editor.host.store().revision();
        let stale = self
            .scene
            .as_ref()
            .is_none_or(|(r, doc, _)| *r != revision || *doc != editor.state.doc);
        if stale {
            let used = used_assets(editor.host.store().workspace(), &editor.state.doc);
            self.scene = Some((revision, editor.state.doc.clone(), used));
        }
        &self.scene.as_ref().expect("filled above").2
    }
}

/// Every asset ID reachable from document `doc` by `@path#id` references.
fn used_assets(workspace: &ara_doc::Workspace, doc: &str) -> BTreeSet<String> {
    fn walk(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(s) if s.starts_with('@') => {
                if let Some((_, id)) = s.rsplit_once('#') {
                    out.push(id.to_owned());
                }
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
            Value::Object(fields) => fields.values().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut used = BTreeSet::new();
    let mut queue = vec![doc.to_owned()];
    while let Some(id) = queue.pop() {
        let Some(document) = workspace.doc(&id) else {
            continue;
        };
        let mut found = Vec::new();
        for record in document.records().values() {
            record.values().for_each(|v| walk(v, &mut found));
        }
        for id in found {
            if used.insert(id.clone()) {
                queue.push(id);
            }
        }
    }
    used
}

impl Shell {
    /// Import progress as `(pending, failed, total)` once the pipeline reports.
    pub(crate) fn import_counts(&self) -> Option<(usize, usize, usize)> {
        let p = self.imports.progress.as_deref()?;
        Some((p.pending.len(), p.failed.len(), p.total))
    }
    /// The status bar's import chip: "Preparing imports…" while the project has
    /// imports and the pipeline has not reported yet, "Importing 37 / 120" with a
    /// bar while sources are pending, else how many failed; nothing once every import
    /// is done.
    pub(super) fn import_chip(&mut self, editor: &Editor, out: &mut String) {
        let Some(progress) = self.imports.progress.clone() else {
            if editor
                .host
                .store()
                .workspace()
                .index()
                .imports()
                .next()
                .is_some()
            {
                self.bind_command("status-imports", "window.assets", Target::None);
                out.push_str(&format!("<button id='status-imports' class='chip' label='Imports' description='Checking which sources need importing; open Assets' action='Dispatch'>{}<text id='status-imports-text' text='Preparing imports…' /></button>",icons::icon_in("status-imports","loader-circle","accent")));
            }
            return;
        };
        if !progress.pending.is_empty() {
            let finished = progress.total - progress.pending.len();
            let percent = finished as f32 * 100. / progress.total.max(1) as f32;
            self.bind_command("status-imports", "window.assets", Target::None);
            out.push_str(&format!("<button id='status-imports' class='chip' label='Imports' description='{} of {} assets still importing; open Assets' action='Dispatch'>{}<text id='status-imports-text' text='Importing {finished} / {}' /><panel id='status-imports-bar' class='progress'><panel id='status-imports-fill' class='progress-fill' style='width:{percent:.0}%' /></panel></button>",progress.pending.len(),progress.total,icons::icon_in("status-imports","loader-circle","accent"),progress.total));
        } else if let Some(error) = progress.failed.values().next() {
            let n = progress.failed.len();
            self.bind_command("status-imports", "window.problems", Target::None);
            out.push_str(&format!("<button id='status-imports' class='chip' label='Imports' description='{}' action='Dispatch'>{}<text id='status-imports-text' class='warning' text='{n} import{} failed' /></button>",escape(error),icons::icon_in("status-imports","triangle-alert","warning"),if n==1{""}else{"s"}));
        }
    }
    /// The Scene view's loading notice while assets the open scene uses are importing
    /// (they draw as placeholders until then) or failed.
    pub(super) fn loading_markup(&mut self, editor: &Editor, out: &mut String) {
        let Some(progress) = self.imports.progress.clone() else {
            return;
        };
        if progress.pending.is_empty() && progress.failed.is_empty() {
            return;
        }
        let used = self.imports.scene_assets(editor);
        let pending = used
            .iter()
            .filter(|id| progress.pending.contains(*id))
            .count();
        let failed = used
            .iter()
            .filter(|id| progress.failed.contains_key(*id))
            .count();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let (text, icon, tint, command) = match (pending, failed) {
            (0, 0) => return,
            (0, f) => (
                format!(
                    "{f} asset{} failed to import; showing placeholders",
                    plural(f)
                ),
                "triangle-alert",
                "warning",
                "window.problems",
            ),
            (p, 0) => (
                format!("Loading {p} asset{}…", plural(p)),
                "loader-circle",
                "accent",
                "window.assets",
            ),
            (p, f) => (
                format!("Loading {p} asset{}… · {f} failed", plural(p)),
                "loader-circle",
                "accent",
                "window.assets",
            ),
        };
        let toolbar = if editor.state.hide_overlays { 0. } else { 36. };
        let top = toolbar
            + if editor.state.follow.is_some() {
                48.
            } else {
                12.
            };
        self.bind_command("scene-loading", command, Target::None);
        out.push_str(&format!("<button id='scene-loading' class='loading-pill' style='top:{top}px' label='Scene assets' description='Assets this scene uses that are not ready yet' action='Dispatch'>{}<text id='scene-loading-text' text='{}' /></button>",icons::icon_in("scene-loading",icon,tint),escape(&text)));
    }
}
