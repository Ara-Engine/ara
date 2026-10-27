//! File imports are typed background commands; this module owns presentation only.
mod backend;
mod jobs;
mod preparation;
mod view;
use super::*;
use ara_ui::dialogs::{DialogFuture, DialogOptions};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

/// One already-resolved authored destination and its absolute OS sources.
pub(crate) struct Request {
    /// Absolute external paths; validation/staging belongs to asset.import.
    pub sources: Vec<PathBuf>,
    /// Authored project-relative folder, resolved from browser storage by the host.
    pub destination: String,
    /// Optional caller hints; fresh read-only preflight replaces these before prompting.
    pub conflicts: Vec<String>,
}
/// Registry actions used by import progress controls.
#[derive(Clone)]
pub(crate) enum Action {
    Cancel(String),
    Dismiss(String),
}

struct Prepared {
    request: Request,
    selection: Selection,
    plan: Value,
}
struct Choice {
    prepared: Prepared,
    response: DialogFuture<Option<usize>>,
}
struct Job {
    id: String,
    destination: String,
    sources: usize,
    status: backend::Status,
    selection: Selection,
    last: Instant,
    cancel_requested: bool,
    preparation: Option<Request>,
    plan: Option<Value>,
    error: Option<String>,
}
struct Selection {
    doc: String,
    entities: Vec<String>,
    asset: Option<String>,
    folder: String,
}
impl Selection {
    fn read(editor: &Editor) -> Self {
        Self {
            doc: editor.state.doc.clone(),
            entities: editor.state.selection.clone(),
            asset: editor.state.asset_path.clone(),
            folder: editor.state.asset_folder.clone(),
        }
    }
    fn unchanged(&self, editor: &Editor) -> bool {
        self.doc == editor.state.doc
            && self.entities == editor.state.selection
            && self.asset == editor.state.asset_path
            && self.folder == editor.state.asset_folder
    }
}
/// Pending choices and authority-owned jobs; no asset files or browser rows are copied here.
#[derive(Default)]
pub(crate) struct State {
    queued: VecDeque<Request>,
    ready: VecDeque<Prepared>,
    choice: Option<Choice>,
    jobs: Vec<Job>,
    /// Memoized Assets markup includes this revision.
    pub revision: u64,
}
impl Shell {
    /// Queue one batch after its target has been resolved to an authored folder.
    /// No selection or authoring state changes until the authority completes it.
    pub(crate) fn queue_asset_import(&mut self, request: Request) -> ara_ui::Result<()> {
        if request.sources.is_empty() {
            return Err("Choose at least one file to import".into());
        }
        if request.sources.iter().any(|path| !path.is_absolute()) {
            return Err("Import sources must be absolute OS paths".into());
        }
        if !backend::authored_folder(&request.destination) {
            return Err("Import destination must be an authored project-relative folder".into());
        }
        self.asset_import.queued.push_back(request);
        self.asset_import.revision += 1;
        Ok(())
    }
    /// Cancel preparation only; the authority owns the final commit boundary.
    pub(crate) fn asset_import_action(
        &mut self,
        editor: &mut Editor,
        action: Action,
    ) -> ara_ui::Result<()> {
        match action {
            Action::Cancel(id) => {
                let status =
                    backend::Status::read(editor.action("jobs.status", json!({"job":id}))?);
                if !status.can_cancel() {
                    self.status_note = Some(if status.running() {
                        "Finishing the import transaction…".into()
                    } else {
                        status.summary()
                    });
                    if let Some(job) = self.asset_import.jobs.iter_mut().find(|job| job.id == id) {
                        job.status = status;
                    }
                    self.asset_import.revision += 1;
                    return Ok(());
                }
                let reply = editor.action("jobs.cancel", json!({"job":id}))?;
                if reply["cancelled"] != true {
                    self.status_note =
                        Some("The import has already finished or is committing".into());
                    self.asset_import.revision += 1;
                    return Ok(());
                }
                if let Some(job) = self.asset_import.jobs.iter_mut().find(|job| job.id == id) {
                    job.cancel_requested = true;
                    job.last = Instant::now() - Duration::from_millis(100);
                }
            }
            Action::Dismiss(id) => self
                .asset_import
                .jobs
                .retain(|job| job.id != id || job.status.running()),
        }
        self.asset_import.revision += 1;
        Ok(())
    }
    /// Preserve the authority's full final path/identity/transaction result for
    /// native verification and the shared Jobs view without copying its schema.
    pub(crate) fn asset_import_report(&self) -> Value {
        json!({"queued":self.asset_import.queued.len(),"ready":self.asset_import.ready.len(),"choosingConflict":self.asset_import.choice.is_some(),
            "conflictPlan":self.asset_import.choice.as_ref().map(|choice|&choice.prepared.plan),
            "jobs":self.asset_import.jobs.iter().map(|job|json!({"job":job.id,"destination":job.destination,
                "sources":job.sources,"record":job.status.value(),"preflight":job.preparation.is_some(),"plan":job.plan,"frontendError":job.error})).collect::<Vec<_>>()})
    }
    /// Browser storage folder under the pointer. The app/backend maps this to an
    /// authored destination before constructing Request; no root names are assumed.
    pub(crate) fn asset_import_storage_folder_at(
        &self,
        editor: &Editor,
        point: [f32; 2],
    ) -> String {
        for &index in self.ui.paint_order.iter().rev() {
            let node = &self.ui.nodes[index];
            if !self.ui.shown(index)
                || !node.rect.contains(point[0], point[1])
                || !node.clip.contains(point[0], point[1])
            {
                continue;
            }
            match self
                .bindings
                .get(&ara_ui::source::stable_id(&node.element.id))
            {
                Some(Binding::AssetFolder(folder, _)) => return folder.clone(),
                Some(Binding::Asset(asset)) if asset.kind == "folder" => return asset.path.clone(),
                Some(Binding::Asset(asset)) => {
                    return asset
                        .path
                        .rsplit_once('/')
                        .map_or(String::new(), |(parent, _)| parent.into());
                }
                _ => {}
            }
        }
        editor.state.asset_folder.clone()
    }
}
