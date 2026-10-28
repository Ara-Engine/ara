//! Migration workers use the special typed bootstrap, never open an old normal host.
use super::*;
use ara_diag::{Diagnostic, codes};
use ara_host::project_layout::{MigrationControl, migrate_project, migration_plan};
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{self, Receiver, Sender};

/// A project layout check or explicitly requested typed migration in the background.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationJob {
    /// Stable worker identifier.
    pub id: String,
    /// Registered project identity.
    pub project: String,
    /// Read-only plan or explicit migrate operation.
    pub kind: String,
    /// running, complete, failed or cancelled.
    pub state: String,
    /// Concise current worker activity.
    pub phase: String,
    /// Whether Cancel can still prevent the typed migration commit.
    pub cancellable: bool,
    /// Fraction of the preparation and physical commit completed.
    pub progress: f32,
    /// Complete plan or typed migration result after physical persistence.
    pub result: Option<Value>,
    /// Catalogued preparation or persistence failures.
    pub diagnostics: Vec<Diagnostic>,
}

struct Entry {
    job: MigrationJob,
    control: MigrationControl,
    cancelled: bool,
}

enum Event {
    Progress(String, f32, String),
    Finished(String, Result<Value, Vec<Diagnostic>>),
}

pub(super) struct Migrations {
    entries: BTreeMap<String, Entry>,
    next: u64,
    send: Sender<Event>,
    events: Receiver<Event>,
}

impl Default for Migrations {
    fn default() -> Self {
        let (send, events) = mpsc::channel();
        Self {
            entries: BTreeMap::new(),
            next: 0,
            send,
            events,
        }
    }
}

impl Migrations {
    pub(super) fn busy(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.job.state == "running")
    }

    pub(super) fn running_for(&self, project: &str) -> bool {
        self.entries.values().any(|entry| {
            entry.job.project == project
                && entry.job.kind == "migrate"
                && entry.job.state == "running"
        })
    }

    fn start(&mut self, project: &ProjectEntry, token: Option<String>) -> Result<Value, RpcError> {
        if self.entries.values().any(|entry| {
            entry.job.project == project.id
                && entry.job.state == "running"
                && entry.job.kind == "migrate"
        }) {
            return Err(invalid("this project already has a migration in progress"));
        }
        self.next += 1;
        let id = format!("project-migration-{}", self.next);
        let control = MigrationControl::default();
        let worker_control = control.clone();
        let root = project.path.clone();
        let send = self.send.clone();
        let worker_id = id.clone();
        let migrating = token.is_some();
        std::thread::Builder::new()
            .name("ara-project-migration".into())
            .spawn(move || {
                super::updates::background_priority();
                let result = match token {
                    Some(token) => {
                        migrate_project(&root, &token, &worker_control, &mut |progress, phase| {
                            let _ = send.send(Event::Progress(
                                worker_id.clone(),
                                progress,
                                phase.to_owned(),
                            ));
                        })
                    }
                    None => migration_plan(&root).and_then(|plan| {
                        serde_json::to_value(plan).map_err(|error| {
                            vec![Diagnostic::error(codes::INVALID_PARAMS, error.to_string())]
                        })
                    }),
                };
                let _ = send.send(Event::Finished(worker_id, result));
            })
            .map_err(io_error)?;
        let job = MigrationJob {
            id: id.clone(),
            project: project.id.clone(),
            kind: if migrating { "migrate" } else { "plan" }.into(),
            state: "running".into(),
            phase: if migrating {
                "Preparing migration"
            } else {
                "Checking project layout"
            }
            .into(),
            cancellable: true,
            progress: 0.,
            result: None,
            diagnostics: Vec::new(),
        };
        self.entries.insert(
            id,
            Entry {
                job: job.clone(),
                control,
                cancelled: false,
            },
        );
        Ok(json!(job))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    project: String,
    plan_token: Option<String>,
    job: Option<String>,
}

impl Hub {
    pub(super) fn poll_migrations(&mut self) {
        let mut changed = BTreeSet::new();
        while let Ok(event) = self.migrations.events.try_recv() {
            let id = match &event {
                Event::Progress(id, ..) | Event::Finished(id, _) => id,
            };
            let Some(entry) = self.migrations.entries.get_mut(id) else {
                continue;
            };
            if entry.job.state != "running" {
                continue;
            }
            match event {
                Event::Progress(_, progress, phase) => {
                    entry.job.progress = progress.clamp(0., 1.);
                    entry.job.phase = phase;
                    entry.job.cancellable = !entry.cancelled && entry.control.cancellable();
                }
                Event::Finished(_, result) => {
                    entry.job.cancellable = false;
                    if entry.cancelled {
                        entry.job.state = "cancelled".into();
                        entry.job.phase = "Cancelled".into();
                    } else {
                        match result {
                            Ok(result) => {
                                entry.job.state = "complete".into();
                                entry.job.phase = "Complete".into();
                                entry.job.progress = 1.;
                                entry.job.result = Some(result);
                            }
                            Err(mut diagnostics) => {
                                if diagnostics.is_empty() {
                                    diagnostics.push(Diagnostic::error(codes::IO,"the migration worker failed without a diagnostic; retry the read-only preview"));
                                }
                                entry.job.state = "failed".into();
                                entry.job.phase = "Could not update the project layout".into();
                                entry.job.diagnostics = diagnostics;
                            }
                        }
                    }
                }
            }
            changed.insert(entry.job.project.clone());
        }
        for project in changed {
            self.changed("migration", Some(&project));
        }
    }

    pub(super) fn migration_call(&mut self, method: &str, value: Value) -> Result<Value, RpcError> {
        let request: Request = parse(value)?;
        let project = self.project(Some(request.project))?;
        match method {
            "hub.project.migration.plan" | "hub.project.migrate" => {
                self.discover_process(&project)?;
                if self.status_of(&project)["state"] == "open" {
                    return Err(invalid(
                        "close this project before changing its folder layout",
                    ));
                }
                let token = if method == "hub.project.migrate" {
                    Some(required(request.plan_token, "planToken")?)
                } else {
                    None
                };
                let result = self.migrations.start(&project, token)?;
                self.changed("migration", Some(&project.id));
                Ok(result)
            }
            "hub.project.migration.status" | "hub.project.migration.cancel" => {
                let id = required(request.job, "job")?;
                let entry = self
                    .migrations
                    .entries
                    .get_mut(&id)
                    .filter(|entry| entry.job.project == project.id)
                    .ok_or_else(|| invalid("unknown project migration job"))?;
                if method == "hub.project.migration.cancel"
                    && entry.job.state == "running"
                    && !entry.cancelled
                {
                    if !entry.control.cancel() {
                        entry.job.cancellable = false;
                        return Err(invalid(
                            "the migration is committing; wait for its files and history to finish saving",
                        ));
                    }
                    entry.cancelled = true;
                    entry.job.cancellable = false;
                    entry.job.phase = "Cancelling".into();
                }
                if entry.job.state == "running" {
                    entry.job.cancellable = !entry.cancelled && entry.control.cancellable();
                }
                Ok(json!(entry.job))
            }
            _ => Err(invalid("unknown project migration method")),
        }
    }
}
