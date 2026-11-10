//! File imports: external files copied into an authored folder by the authority's typed
//! `asset.import` (a background job), after a read-only preflight names the files that
//! already exist there, for the person to rename, replace or skip ([`Importer`]).
pub(crate) mod backend;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::Editor;
use backend::{Preflight, Status};

/// What existing files a batch would overwrite: the person picks a policy
/// ([`Importer::resolve`]) before anything is imported.
pub(crate) struct Conflicts {
    sources: Vec<PathBuf>,
    destination: String,
    /// One line per existing file (`rock.glb → Art/rock.glb (file)`).
    pub labels: Vec<String>,
}

/// One batch's job: its preflight (while `plan` holds the batch) or its import.
struct Job {
    id: String,
    status: Status,
    last: Instant,
    plan: Option<(Vec<PathBuf>, String)>,
}

/// The window's imports: batches waiting on their preflight, a conflict choice, imports
/// running. Polled each pass ([`Self::poll`]).
#[derive(Default)]
pub(crate) struct Importer {
    jobs: Vec<Job>,
    /// A batch whose files already exist, waiting for Rename, Replace or Skip.
    pub conflicts: Option<Conflicts>,
    /// How the last import ended, for the window to show.
    pub note: Option<String>,
}

impl Importer {
    /// Imports `sources` (absolute OS paths) into authored folder `destination`: checks
    /// first, and imports at once unless files there already exist.
    pub fn import(
        &mut self,
        editor: &mut Editor,
        sources: Vec<PathBuf>,
        destination: String,
    ) -> ara_ui::Result<()> {
        if sources.is_empty() {
            return Err("Choose at least one file to import".into());
        }
        if sources.iter().any(|path| !path.is_absolute()) {
            return Err("Import sources must be absolute paths".into());
        }
        if !backend::authored_folder(&destination) {
            return Err("Import into an authored project folder".into());
        }
        match backend::preflight(editor, &sources, &destination)? {
            Preflight::Ready(plan) => self.planned(editor, sources, destination, plan.labels()),
            Preflight::Pending(id) => {
                self.jobs.push(Job {
                    id,
                    status: Status::preparing(),
                    last: Instant::now() - Duration::from_millis(100),
                    plan: Some((sources, destination)),
                });
                Ok(())
            }
        }
    }

    fn planned(
        &mut self,
        editor: &mut Editor,
        sources: Vec<PathBuf>,
        destination: String,
        labels: Vec<String>,
    ) -> ara_ui::Result<()> {
        if labels.is_empty() {
            return self.start(editor, sources, destination, "rename");
        }
        self.conflicts = Some(Conflicts {
            sources,
            destination,
            labels,
        });
        Ok(())
    }

    /// Imports the conflicting batch with `policy` (`rename` keeps both, `replace` updates
    /// the existing assets, `skip` leaves them), or with none drops it.
    pub fn resolve(&mut self, editor: &mut Editor, policy: Option<&str>) -> ara_ui::Result<()> {
        let Some(batch) = self.conflicts.take() else {
            return Ok(());
        };
        match policy {
            Some(policy @ ("rename" | "replace" | "skip")) => {
                self.start(editor, batch.sources, batch.destination, policy)
            }
            Some(_) => Err("Rename, replace or skip".into()),
            None => {
                self.note = Some("Import cancelled".into());
                Ok(())
            }
        }
    }

    fn start(
        &mut self,
        editor: &mut Editor,
        sources: Vec<PathBuf>,
        destination: String,
        conflict: &str,
    ) -> ara_ui::Result<()> {
        let paths = sources
            .iter()
            .map(|path| path.to_str().ok_or("A file's path is not UTF-8"))
            .collect::<Result<Vec<_>, _>>()?;
        let why = match paths.len() {
            1 => "Import 1 file".to_owned(),
            n => format!("Import {n} files"),
        };
        // Imports belong to the project marker, even when a scene is open.
        let accepted = editor.action(
            "cmd.apply",
            json!({"doc": "ara.project.json", "why": why, "commands": [{
                "cmd": "asset.import",
                "args": {"sources": paths, "destination": destination, "conflict": conflict}
            }]}),
        )?;
        if accepted["ok"] == false {
            return Err("The import was not accepted".into());
        }
        let id = backend::accepted_job(&accepted)
            .ok_or("The import did not start its job")?
            .to_owned();
        self.jobs.push(Job {
            id,
            status: Status::preparing(),
            last: Instant::now() - Duration::from_millis(100),
            plan: None,
        });
        Ok(())
    }

    /// The first running import's message and progress (0–1).
    pub fn progress(&self) -> Option<(&str, f32)> {
        self.jobs
            .iter()
            .find(|job| job.status.running())
            .map(|job| (job.status.message.as_str(), job.status.progress))
    }

    /// Whether a running import can still be cancelled (before it commits).
    pub fn can_cancel(&self) -> bool {
        self.jobs.iter().any(|job| job.status.can_cancel())
    }

    /// Asks every cancellable import to stop; the authority keeps the commit boundary.
    pub fn cancel(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        for job in self.jobs.iter().filter(|job| job.status.can_cancel()) {
            editor.action("jobs.cancel", json!({"job": job.id}))?;
        }
        Ok(())
    }

    /// Reads each running job's status (at most every 100 ms): a finished preflight
    /// imports or asks about conflicts, a finished import selects what it made. Whether
    /// anything changed.
    pub fn poll(&mut self, editor: &mut Editor) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.jobs.len() {
            let job = &mut self.jobs[i];
            if !job.status.running() || job.last.elapsed() < Duration::from_millis(100) {
                i += 1;
                continue;
            }
            job.last = Instant::now();
            let status = match editor.action("jobs.status", json!({"job": job.id})) {
                Ok(raw) => Status::read(raw),
                Err(error) => {
                    Status::read(json!({"status": "failed", "message": error.to_string()}))
                }
            };
            if status == job.status {
                i += 1;
                continue;
            }
            changed = true;
            job.status = status;
            if job.status.running() {
                i += 1;
                continue;
            }
            let job = self.jobs.remove(i);
            let result = match (job.plan, job.status.succeeded()) {
                (Some((sources, destination)), true) => {
                    backend::Plan::read(&job.status.value()["result"])
                        .and_then(|plan| self.planned(editor, sources, destination, plan.labels()))
                }
                (None, true) => {
                    self.note = Some(job.status.summary());
                    match job.status.selection_path() {
                        Some(path) => editor
                            .action("editor.workspace", json!({"asset": path}))
                            .map(drop)
                            .map_err(Into::into),
                        None => Ok(()),
                    }
                }
                (_, false) => {
                    self.note = Some(job.status.summary());
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.note = Some(error.to_string());
            }
        }
        changed
    }
}
