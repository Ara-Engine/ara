//! Explicit consent to replace an incompatible project pin with the current engine.
use super::*;
use ara_hub_service::MigrationJob;
use serde_json::json;

/// The step owning the compatibility dialog and its pending open request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Read-only installed-engine capability check.
    Checking,
    /// Awaiting Use current engine / Install current engine / Cancel.
    Decision,
    /// The existing signed installation flow owns its confirmation and progress.
    Installing,
    /// An explicitly accepted typed pin is saving files and history.
    Pinning,
}

/// A project open paused because its selected engine cannot read content/.
pub struct Flow {
    /// Project whose marker may be changed after consent.
    pub project: String,
    /// Resume the originally requested open mode.
    pub headless: bool,
    /// Current consent/progress stage.
    pub phase: Phase,
    /// Current engine package version, supplied by the service.
    pub version: String,
    /// A capable CLI and the requested editor/headless executable are installed.
    pub available: bool,
    /// Whether a same-version registration already exists.
    pub registered: bool,
    /// Latest typed pin persistence snapshot.
    pub job: Option<MigrationJob>,
    request: u64,
    status_pending: bool,
    consented: bool,
    install_job: Option<String>,
}

impl Hub {
    pub(super) fn review_engine_layout(&mut self, project: String, headless: bool) {
        if self.engine_layout.is_some() || self.migration.is_some() {
            return;
        }
        self.next_engine_layout += 1;
        self.menu = None;
        self.busy.insert(project.clone(), Busy::Inspecting);
        self.engine_layout = Some(Flow {
            project: project.clone(),
            headless,
            phase: Phase::Checking,
            version: String::new(),
            available: false,
            registered: false,
            job: None,
            request: self.next_engine_layout,
            status_pending: false,
            consented: false,
            install_job: None,
        });
        if self.dialog.is_none() {
            self.dialog = Some(Dialog::EngineLayout(project));
        }
        self.check_current_engine();
    }

    fn check_current_engine(&mut self) {
        let Some(flow) = &mut self.engine_layout else {
            return;
        };
        flow.phase = Phase::Checking;
        let call = Call::ProjectEngine {
            project: flow.project.clone(),
            request: flow.request,
            method: "hub.project.currentEngine",
            params: json!({"headless":flow.headless}),
        };
        self.send(call);
    }

    pub(super) fn accept_current_engine(&mut self) {
        let Some(flow) = &mut self.engine_layout else {
            return;
        };
        if flow.phase != Phase::Decision {
            return;
        }
        flow.consented = true;
        if flow.available {
            self.pin_current_engine();
        } else {
            flow.phase = Phase::Installing;
            let version = flow.version.clone();
            self.releases.jobs.retain(|job| {
                !(job.kind == "engine-current" && job.version == version && job.state == "complete")
            });
            let dialog = Dialog::InstallCurrentEngine(version);
            self.intent(Intent::Dialog(dialog));
        }
    }

    fn pin_current_engine(&mut self) {
        let Some(flow) = &mut self.engine_layout else {
            return;
        };
        flow.phase = Phase::Pinning;
        self.busy.insert(flow.project.clone(), Busy::Saving);
        if self.dialog.is_none()
            || matches!(&self.dialog,Some(Dialog::EngineLayout(project)) if project == &flow.project)
            || matches!(&self.dialog,Some(Dialog::InstallCurrentEngine(version)) if version == &flow.version)
        {
            self.dialog = Some(Dialog::EngineLayout(flow.project.clone()));
        }
        let call = Call::ProjectEngine {
            project: flow.project.clone(),
            request: flow.request,
            method: "hub.project.useCurrentEngine",
            params: json!({"headless":flow.headless}),
        };
        self.send(call);
    }

    pub(super) fn cancel_engine_layout(&mut self) {
        if self
            .engine_layout
            .as_ref()
            .is_some_and(|flow| flow.phase == Phase::Pinning)
        {
            return;
        }
        self.finish_engine_layout(false);
    }

    fn finish_engine_layout(&mut self, open: bool) {
        let Some(flow) = self.engine_layout.take() else {
            return;
        };
        self.busy.remove(&flow.project);
        if matches!(&self.dialog,Some(Dialog::EngineLayout(project)) if project == &flow.project)
            || matches!(&self.dialog,Some(Dialog::InstallCurrentEngine(version)) if version == &flow.version)
        {
            self.dialog = None;
        }
        if open {
            self.refresh([List::Projects]);
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

    pub(super) fn engine_layout_install_started(&mut self, job: &updates::Download) {
        if let Some(flow) = &mut self.engine_layout
            && flow.phase == Phase::Installing
            && job.kind == "engine-current"
            && flow.version == job.version
        {
            flow.install_job = Some(job.id.clone());
        }
    }

    pub(super) fn engine_layout_owns_install(&self, id: &str) -> bool {
        self.engine_layout
            .as_ref()
            .is_some_and(|flow| flow.install_job.as_deref() == Some(id))
    }

    pub(super) fn engine_layout_installed(&mut self, version: &str, id: &str) {
        if self.engine_layout.as_ref().is_some_and(|flow| {
            flow.phase == Phase::Installing
                && flow.version == version
                && flow.install_job.as_deref() == Some(id)
        }) {
            self.check_current_engine();
        }
    }

    pub(super) fn engine_layout_cancelled(&mut self, version: &str) {
        if self
            .engine_layout
            .as_ref()
            .is_some_and(|flow| flow.phase == Phase::Installing && flow.version == version)
        {
            self.cancel_engine_layout();
        }
    }

    pub(super) fn engine_layout_transfer_failed(&mut self, version: &str) -> Option<Intent> {
        let flow = self
            .engine_layout
            .as_mut()
            .filter(|flow| flow.phase == Phase::Installing && flow.version == version)?;
        flow.phase = Phase::Decision;
        flow.consented = false;
        Some(Intent::Dialog(Dialog::EngineLayout(flow.project.clone())))
    }

    pub(super) fn dismiss_engine_install(&mut self, version: &str) {
        if !self.releases.starting("engine-current", version)
            && self.releases.running("engine-current", version).is_none()
            && !self.releases.starting("engine", version)
            && !self.releases.starting("repair", version)
            && self.releases.running("engine", version).is_none()
            && self.releases.running("repair", version).is_none()
        {
            self.engine_layout_cancelled(version);
        }
    }

    pub(super) fn refresh_engine_layout(&mut self) {
        let Some(flow) = &mut self.engine_layout else {
            return;
        };
        let Some(job) = flow.job.as_ref().filter(|job| job.state == "running") else {
            return;
        };
        if flow.status_pending {
            return;
        }
        flow.status_pending = true;
        let call = Call::ProjectEngine {
            project: flow.project.clone(),
            request: flow.request,
            method: "hub.project.migration.status",
            params: json!({"job":job.id}),
        };
        self.send(call);
    }

    pub(super) fn engine_layout_reply(
        &mut self,
        project: String,
        request: u64,
        method: &'static str,
        result: Result<Value, RpcError>,
    ) {
        if !self
            .engine_layout
            .as_ref()
            .is_some_and(|flow| flow.project == project && flow.request == request)
        {
            return;
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.engine_layout_failed(error);
                return;
            }
        };
        if method == "hub.project.currentEngine" {
            let Some(version) = value["engineVersion"]
                .as_str()
                .filter(|version| !version.is_empty())
            else {
                self.engine_layout_failed(
                    Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "the service returned no current engine version",
                    )
                    .into(),
                );
                return;
            };
            let available = value["available"] == true;
            let Some(flow) = &mut self.engine_layout else {
                return;
            };
            flow.version = version.into();
            flow.available = available;
            flow.registered = value["registered"] == true;
            if flow.consented {
                if available {
                    self.pin_current_engine();
                } else {
                    self.engine_layout_failed(Diagnostic::error(codes::PROJECT_ENGINE_LAYOUT_UNSUPPORTED,"The installed engine does not declare support for the content/ layout; install Ara preview 4 or newer before opening this project").into());
                }
            } else {
                flow.phase = Phase::Decision;
            }
            return;
        }
        let job: MigrationJob = match serde_json::from_value(value) {
            Ok(job) => job,
            Err(error) => {
                self.engine_layout_failed(
                    Diagnostic::error(codes::INVALID_PARAMS, error.to_string()).into(),
                );
                return;
            }
        };
        if let Some(flow) = &mut self.engine_layout {
            flow.status_pending = false;
        }
        match job.state.as_str() {
            "complete" => self.finish_engine_layout(true),
            "failed" => self.engine_layout_failed(RpcError {
                code: -32000,
                message: "Could not save the engine choice".into(),
                data: Some(json!({"diagnostics":job.diagnostics})),
            }),
            "cancelled" => self.finish_engine_layout(false),
            _ => {
                if let Some(flow) = &mut self.engine_layout {
                    flow.job = Some(job);
                }
                if method != "hub.project.migration.status" {
                    self.refresh_engine_layout();
                }
            }
        }
    }

    fn engine_layout_failed(&mut self, error: RpcError) {
        self.finish_engine_layout(false);
        self.notice("Couldn't use the current engine".into(), Some(&error));
    }
}
