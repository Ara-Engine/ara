//! Read-only uninstall inspection is separate from the explicitly confirmed deletion.
use super::*;

/// The exact managed destination and approximate disk space affected by uninstall.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalSummary {
    /// Engine version inspected by the service.
    pub version: String,
    /// Exact installation to remove.
    pub destination: PathBuf,
    /// File bytes currently occupying the installation, before filesystem overhead.
    pub bytes: u64,
    /// Number of files inspected.
    pub files: u64,
}

impl Hub {
    pub(in crate::model) fn inspect_removal(&mut self, version: String) {
        self.releases.removal_summaries.remove(&version);
        let closed = self.releases.closed_removals.remove(&version);
        if closed
            && (self.releases.starting("uninstall-preflight", &version)
                || self
                    .releases
                    .running("uninstall-preflight", &version)
                    .is_some())
        {
            self.releases.restart_removals.insert(version);
            return;
        }
        if self.releases.starting("uninstall-preflight", &version)
            || self
                .releases
                .running("uninstall-preflight", &version)
                .is_some()
        {
            return;
        }
        self.releases
            .jobs
            .retain(|job| job.kind != "uninstall-preflight" || job.version != version);
        self.start_transfer(
            "uninstall-preflight",
            version,
            "hub.engine.uninstall.preflight",
        );
    }

    pub(in crate::model) fn close_removal(&mut self, version: &str) {
        self.releases.closed_removals.insert(version.into());
        self.releases.restart_removals.remove(version);
        self.releases.removal_summaries.remove(version);
        if self.releases.starting("uninstall-preflight", version) {
            self.cancel_pending("uninstall-preflight".into(), version.into());
        } else if let Some(job) = self.releases.running("uninstall-preflight", version) {
            let id = job.id.clone();
            self.release_call("hub.download.cancel", json!({"job":id}));
        }
    }

    fn removal_active(&self, version: &str) -> bool {
        !self.releases.closed_removals.contains(version)
            && matches!(&self.dialog, Some(Dialog::UninstallEngine(engine)) if engine.version == version)
    }

    pub(in crate::model) fn removal_cancel(&self, job: &str) -> bool {
        self.releases.removal_jobs.contains(job)
    }

    pub(in crate::model) fn removal_error(&mut self, version: &str, error: &RpcError) {
        if !self.removal_active(version) {
            return;
        }
        let Some(Dialog::UninstallEngine(engine)) = self.dialog.take() else {
            return;
        };
        let id = self.notice(format!("Couldn't inspect Ara {version}"), Some(error));
        if let Some(notice) = self.notices.iter_mut().find(|notice| notice.id == id) {
            notice.retry = Some(Box::new(Intent::Dialog(Dialog::UninstallEngine(engine))));
        }
    }

    pub(in crate::model) fn receive_removal(&mut self, job: &Download) {
        if job.kind != "uninstall-preflight" {
            return;
        }
        self.releases.removal_jobs.insert(job.id.clone());
        if !self.removal_active(&job.version)
            || self.releases.restart_removals.contains(&job.version)
            || self.releases.starting("uninstall-preflight", &job.version)
        {
            return;
        }
        if let Some(error) = &job.error {
            self.removal_error(&job.version, &error.clone().into());
        } else if matches!(job.state.as_str(), "cancelled" | "failed") {
            self.removal_error(&job.version, &Diagnostic::error(codes::IO, "The installation inspection did not finish. Retry the space check before uninstalling.").into());
        } else if job.state == "complete" {
            let summary = job
                .result
                .as_ref()
                .ok_or("the inspection returned no summary")
                .and_then(|result| {
                    serde_json::from_value::<RemovalSummary>(result.clone())
                        .map_err(|_| "the inspection returned an invalid summary")
                });
            match summary {
                Ok(summary) if summary.version == job.version && !summary.destination.as_os_str().is_empty() => { self.releases.removal_summaries.insert(job.version.clone(), summary); }
                _ => self.removal_error(&job.version, &Diagnostic::error(codes::INVALID_PARAMS, "The installation inspection returned an invalid destination or space summary. Retry the inspection.").into()),
            }
        }
    }

    pub(in crate::model) fn restart_removal_checks(&mut self) {
        let versions: Vec<_> = self
            .releases
            .restart_removals
            .iter()
            .filter(|version| {
                self.removal_active(version)
                    && !self.releases.starting("uninstall-preflight", version)
                    && self.releases.jobs.iter().any(|job| {
                        job.kind == "uninstall-preflight"
                            && &job.version == *version
                            && job.state != "running"
                    })
            })
            .cloned()
            .collect();
        for version in versions {
            self.releases.restart_removals.remove(&version);
            self.inspect_removal(version);
        }
    }

    pub(in crate::model) fn removal_request_failed(&mut self, version: &str, error: &RpcError) {
        if self.releases.restart_removals.remove(version) && self.removal_active(version) {
            self.inspect_removal(version.into());
        } else {
            self.removal_error(version, error);
        }
    }
}
