//! Consent and progress for opening a project whose old layout must be migrated.
use super::*;
use ara_hub_service::MigrationJob;
use serde_json::json;

/// Read-only migration preview returned by the shared project worker.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// False when a refreshed project already uses the current layout.
    pub needs_migration: bool,
    /// Layout version on disk before the move.
    pub from_version: u64,
    /// Layout version after the move.
    pub to_version: u64,
    /// Single authored-content directory.
    pub authored_root: String,
    /// Immutable preview identity revalidated by the typed migration command.
    pub plan_token: String,
    /// Ordered move and reference-rewrite descriptions.
    #[serde(default)]
    pub operations: Vec<Operation>,
    /// Destination paths shown in the consent dialog.
    #[serde(default)]
    pub preview_tree: Vec<String>,
}

/// One project-relative planned operation; this is presentation data only.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Operation {
    /// Move existing content without changing asset identities.
    Move { from: String, to: String },
    /// Update the canonical file's references or project settings.
    Rewrite { path: String },
}

/// Which step owns the migration dialog's controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Read-only background inspection.
    Checking,
    /// The user must explicitly choose Migrate or Cancel.
    Decision,
    /// Explicitly authorized preparation and typed persistence.
    Migrating,
    /// Cancellation is awaiting the worker's commit-boundary decision.
    Cancelling,
}

/// One requested project open, paused at its migration consent boundary.
pub struct Flow {
    /// Registered project identity.
    pub project: String,
    /// Continue in the originally requested mode after successful migration.
    pub headless: bool,
    /// Current consent/progress step.
    pub phase: Phase,
    /// Frozen read-only preview.
    pub plan: Option<Plan>,
    /// Latest background worker snapshot.
    pub job: Option<MigrationJob>,
    request: u64,
    status_pending: bool,
}

impl Flow {
    /// Cancel is always safe before consent, and later follows the typed commit gate.
    pub fn cancellable(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Decision)
            || (self.phase == Phase::Migrating
                && self.job.as_ref().is_some_and(|job| job.cancellable))
    }
}

impl Hub {
    pub(super) fn review_migration(&mut self, project: String, headless: bool) {
        if self.migration.is_some() {
            return;
        }
        self.next_migration += 1;
        let request = self.next_migration;
        self.menu = None;
        self.busy.insert(project.clone(), Busy::Inspecting);
        if let Some(Dialog::Error(id)) = &self.dialog {
            self.notices.retain(|notice| notice.id != *id);
            self.dialog = None;
        }
        if self.dialog.is_none() {
            self.dialog = Some(Dialog::Migration(project.clone()));
        }
        self.migration = Some(Flow {
            project: project.clone(),
            headless,
            phase: Phase::Checking,
            plan: None,
            job: None,
            request,
            status_pending: false,
        });
        self.send(Call::Migration {
            project,
            request,
            method: "hub.project.migration.plan",
            params: json!({}),
        });
    }

    pub(super) fn migrate_project(&mut self) {
        let Some(flow) = &mut self.migration else {
            return;
        };
        if flow.phase != Phase::Decision {
            return;
        }
        let Some(plan) = &flow.plan else { return };
        flow.phase = Phase::Migrating;
        flow.job = None;
        self.busy.insert(flow.project.clone(), Busy::Migrating);
        let call = Call::Migration {
            project: flow.project.clone(),
            request: flow.request,
            method: "hub.project.migrate",
            params: json!({"planToken":plan.plan_token}),
        };
        self.send(call);
    }

    pub(super) fn cancel_migration(&mut self) {
        let Some(flow) = &mut self.migration else {
            return;
        };
        if !flow.cancellable() {
            return;
        }
        let cancel = flow
            .job
            .as_ref()
            .filter(|job| job.state == "running")
            .map(|job| Call::Migration {
                project: flow.project.clone(),
                request: flow.request,
                method: "hub.project.migration.cancel",
                params: json!({"job":job.id}),
            });
        if flow.phase == Phase::Migrating {
            flow.phase = Phase::Cancelling;
        } else {
            self.finish_migration(false);
        }
        if let Some(cancel) = cancel {
            self.send(cancel);
        }
    }

    fn finish_migration(&mut self, open: bool) {
        let Some(flow) = self.migration.take() else {
            return;
        };
        self.busy.remove(&flow.project);
        if matches!(&self.dialog, Some(Dialog::Migration(project)) if project == &flow.project) {
            self.dialog = None;
        }
        if open {
            self.lifecycle(
                flow.project,
                if flow.headless {
                    Busy::Starting
                } else {
                    Busy::Opening
                },
            );
        }
    }

    pub(super) fn refresh_migration(&mut self) {
        let Some(flow) = &mut self.migration else {
            return;
        };
        if flow.status_pending {
            return;
        }
        let Some(job) = flow.job.as_ref().filter(|job| job.state == "running") else {
            return;
        };
        flow.status_pending = true;
        let call = Call::Migration {
            project: flow.project.clone(),
            request: flow.request,
            method: "hub.project.migration.status",
            params: json!({"job":job.id}),
        };
        self.send(call);
    }

    pub(super) fn migration_reply(
        &mut self,
        project: String,
        request: u64,
        method: &'static str,
        result: Result<Value, RpcError>,
    ) {
        if !self
            .migration
            .as_ref()
            .is_some_and(|flow| flow.project == project && flow.request == request)
        {
            if method == "hub.project.migration.plan"
                && let Ok(value) = result
                && let Ok(job) = serde_json::from_value::<MigrationJob>(value)
                && job.state == "running"
            {
                self.send(Call::Migration {
                    project,
                    request,
                    method: "hub.project.migration.cancel",
                    params: json!({"job":job.id}),
                });
            }
            return;
        }
        if let Some(flow) = &mut self.migration {
            flow.status_pending = false;
        }
        let value = match result {
            Ok(value) => value,
            Err(_) if method == "hub.project.migration.cancel" => {
                if let Some(flow) = &mut self.migration {
                    flow.phase = Phase::Migrating;
                    if let Some(job) = &mut flow.job {
                        job.cancellable = false;
                        job.phase = "Finishing files and history".into();
                    }
                }
                self.refresh_migration();
                return;
            }
            Err(error) => {
                self.migration_failed(error);
                return;
            }
        };
        let job: MigrationJob = match serde_json::from_value(value) {
            Ok(job) => job,
            Err(error) => {
                self.migration_failed(
                    Diagnostic::error(codes::INVALID_PARAMS, error.to_string()).into(),
                );
                return;
            }
        };
        match job.state.as_str() {
            "complete" if job.kind == "plan" => {
                let plan = job
                    .result
                    .clone()
                    .ok_or_else(|| "the migration check returned no preview".to_owned())
                    .and_then(|value| {
                        serde_json::from_value::<Plan>(value).map_err(|error| error.to_string())
                    });
                match plan {
                    Ok(plan) if !plan.needs_migration => self.finish_migration(true),
                    Ok(plan) => {
                        if let Some(flow) = &mut self.migration {
                            flow.plan = Some(plan);
                            flow.phase = Phase::Decision;
                            flow.job = Some(job);
                        }
                    }
                    Err(error) => self
                        .migration_failed(Diagnostic::error(codes::INVALID_PARAMS, error).into()),
                }
            }
            "complete" => self.finish_migration(true),
            "cancelled" => self.finish_migration(false),
            "failed" => {
                let error = RpcError {
                    code: -32000,
                    message: "Project migration failed".into(),
                    data: Some(json!({"diagnostics":job.diagnostics})),
                };
                self.migration_failed(error);
            }
            _ => {
                if let Some(flow) = &mut self.migration {
                    flow.job = Some(job);
                }
                if method != "hub.project.migration.status" {
                    self.refresh_migration();
                }
            }
        }
    }

    fn migration_failed(&mut self, error: RpcError) {
        let retry = self.migration.as_ref().map(|flow| Intent::ReviewMigration {
            project: flow.project.clone(),
            headless: flow.headless,
        });
        self.finish_migration(false);
        let id = self.notice("Couldn't update the project layout".into(), Some(&error));
        if let Some(notice) = self.notices.iter_mut().find(|notice| notice.id == id) {
            notice.retry = retry.map(Box::new);
        }
    }
}
