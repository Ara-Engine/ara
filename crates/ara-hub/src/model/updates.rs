//! Release view state uses the service's background jobs, including install-and-open.
use super::*;
use ara_update::{Channel, Manifest};
use serde_json::json;
mod removal;
pub use removal::RemovalSummary;

/// The service's asynchronous transfer snapshot.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Download {
    /// Stable job identifier, used for cancellation.
    pub id: String,
    /// engine or hub.
    pub kind: String,
    /// Signed version.
    pub version: String,
    /// running, complete, cancelled or failed.
    pub state: String,
    /// Download, signature/hash verification, extraction or installation phase.
    pub phase: String,
    /// False after an atomic commit, when cancelling could no longer preserve the installation.
    #[serde(default = "cancellable")]
    pub cancellable: bool,
    /// Disk-space preflight for the volume receiving the installation.
    pub disk: Option<Disk>,
    /// Streamed bytes and signed total.
    pub progress: Option<ara_update::Progress>,
    /// Installed build or authenticated Hub staging result.
    pub result: Option<Value>,
    /// Catalogued failure.
    pub error: Option<Diagnostic>,
}
fn cancellable() -> bool {
    true
}
/// Space needed by the signed transfer and its staged installation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Disk {
    /// Free bytes on the destination volume.
    pub available_bytes: u64,
    /// Required bytes including the staged payload.
    pub required_bytes: u64,
}
/// A read-only space and destination check before the user starts a transfer.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preflight {
    /// Destination of the staged engine installation.
    pub destination: PathBuf,
    /// Signed archive download size.
    pub download_bytes: u64,
    /// Estimated installation/staging space, rechecked exactly after download.
    pub required_bytes: u64,
    /// Free bytes on the installation volume at the time of the check.
    pub available_bytes: u64,
    /// Selected stable or preview channel.
    pub channel: Option<Channel>,
}
/// Data rendered by Installs and Settings.
#[derive(Debug)]
pub struct Releases {
    /// Standalone installs permit a user-requested Hub update.
    pub can_self_update: bool,
    /// Authenticated published engine builds.
    pub available: Vec<Manifest>,
    /// Latest compatible Hub update newer than the running GUI.
    pub hub: Option<Manifest>,
    /// Transfer snapshots.
    pub jobs: Vec<Download>,
    /// Selected channel.
    pub channel: Channel,
    /// Repository setting/draft.
    pub repo: String,
    /// Managed engine installation directory reported by the service.
    pub install_root: Option<PathBuf>,
    /// Destination/space summaries requested from explicit install dialogs.
    pub plans: BTreeMap<String, Preflight>,
    /// Feed freshness or diagnostic message.
    pub status: String,
    /// A check is in flight.
    pub checking: bool,
    pub(super) open_after: BTreeMap<String, BTreeMap<String, bool>>,
    pending_installs: BTreeSet<String>,
    attempted_updates: BTreeSet<String>,
    pending_preflights: BTreeSet<String>,
    requests: BTreeSet<(String, String)>,
    cancelling_requests: BTreeSet<(String, String)>,
    removal_summaries: BTreeMap<String, RemovalSummary>,
    closed_removals: BTreeSet<String>,
    restart_removals: BTreeSet<String>,
    removal_jobs: BTreeSet<String>,
}
impl Default for Releases {
    fn default() -> Self {
        Self {
            can_self_update: false,
            available: Vec::new(),
            hub: None,
            jobs: Vec::new(),
            channel: Channel::Stable,
            repo: ara_update::DEFAULT_REPOSITORY.into(),
            install_root: None,
            plans: BTreeMap::new(),
            status: "Check for published versions".into(),
            checking: false,
            open_after: BTreeMap::new(),
            pending_installs: BTreeSet::new(),
            attempted_updates: BTreeSet::new(),
            pending_preflights: BTreeSet::new(),
            requests: BTreeSet::new(),
            cancelling_requests: BTreeSet::new(),
            removal_summaries: BTreeMap::new(),
            closed_removals: BTreeSet::new(),
            restart_removals: BTreeSet::new(),
            removal_jobs: BTreeSet::new(),
        }
    }
}
impl Releases {
    /// Read-only deletion scope and space result for the current inspection.
    pub fn removal(&self, version: &str) -> Option<&RemovalSummary> {
        self.removal_summaries.get(version)
    }
    /// The service has not answered the transfer's start request yet.
    pub fn starting(&self, kind: &str, version: &str) -> bool {
        self.requests.contains(&(kind.into(), version.into()))
            || (kind == "engine" && self.pending_installs.contains(version))
    }
    /// Cancellation will be forwarded as soon as the service supplies the job ID.
    pub fn cancelling(&self, kind: &str, version: &str) -> bool {
        self.cancelling_requests
            .contains(&(kind.into(), version.into()))
    }
    /// A running transfer for this release.
    pub fn running(&self, kind: &str, version: &str) -> Option<&Download> {
        self.jobs.iter().find(|j| {
            j.kind == kind
                && j.version == version
                && j.state == "running"
                && (kind != "hub" || self.hub.as_ref().is_some_and(|hub| matches_release(j, hub)))
        })
    }
    /// Verified Hub archive ready for the external updater.
    pub fn prepared(&self) -> Option<&Value> {
        self.jobs
            .iter()
            .find(|j| {
                j.kind == "hub"
                    && j.state == "complete"
                    && self.hub.as_ref().is_some_and(|hub| matches_release(j, hub))
            })
            .and_then(|j| j.result.as_ref())
    }
}

/// Identity includes the source generation so skipping a preview does not hide its replacement.
pub fn identity(release: &Manifest) -> String {
    match &release.build {
        Some(build) => format!("{}:{}", release.version, build.commit),
        None => release.version.clone(),
    }
}
fn matches_release(job: &Download, release: &Manifest) -> bool {
    let id = match &release.build {
        Some(build) => format!("hub-{}-{}", release.version, build.commit),
        None => format!("hub-{}", release.version),
    };
    job.id == id
}

impl Hub {
    pub(super) fn start_transfer(&mut self, kind: &str, version: String, method: &'static str) {
        if self.releases.running(kind, &version).is_none()
            && self
                .releases
                .requests
                .insert((kind.into(), version.clone()))
        {
            self.release_call(
                method,
                json!({"version":version,"hubVersion":env!("CARGO_PKG_VERSION")}),
            );
        }
    }
    pub(super) fn cancel_pending(&mut self, kind: String, version: String) {
        if matches!(kind.as_str(), "engine" | "engine-current" | "repair") {
            self.engine_layout_cancelled(&version);
        }
        if kind == "engine" {
            self.releases.open_after.remove(&version);
        }
        if kind == "engine" && self.releases.pending_installs.remove(&version) {
            self.releases.open_after.remove(&version);
            return;
        }
        let key = (kind, version);
        if self.releases.requests.contains(&key) {
            self.releases.cancelling_requests.insert(key);
        }
    }
    pub(super) fn transfer_failed(&mut self, method: &str, version: &str) {
        let kind = match method {
            "hub.engine.install" => "engine",
            "hub.engine.installCurrent" => "engine-current",
            "hub.engine.repair" => "repair",
            "hub.update.prepare" => "hub",
            "hub.engine.uninstall" => "uninstall",
            "hub.engine.uninstall.preflight" => "uninstall-preflight",
            _ => return,
        };
        let key = (kind.into(), version.into());
        self.releases.requests.remove(&key);
        self.releases.cancelling_requests.remove(&key);
        if kind == "engine" {
            self.releases.open_after.remove(version);
        }
    }
    pub(super) fn preflight_engine(&mut self, version: String) {
        if !self.releases.plans.contains_key(&version)
            && self.releases.pending_preflights.insert(version.clone())
        {
            self.release_call("hub.engine.preflight", json!({"version":version}));
        }
    }
    pub(super) fn preflight_reply(&mut self, version: String, result: Result<Value, RpcError>) {
        self.releases.pending_preflights.remove(&version);
        let visible = matches!(&self.dialog, Some(Dialog::InstallEngine(current) | Dialog::InstallCurrentEngine(current) | Dialog::RepairEngine(current)) if current == &version);
        let retry = if matches!(&self.dialog, Some(Dialog::InstallCurrentEngine(current)) if current == &version)
        {
            Dialog::InstallCurrentEngine(version.clone())
        } else if matches!(&self.dialog, Some(Dialog::RepairEngine(current)) if current == &version)
        {
            Dialog::RepairEngine(version.clone())
        } else {
            Dialog::InstallEngine(version.clone())
        };
        let result = result.and_then(|value| {
            serde_json::from_value::<Preflight>(value).map_err(|error| {
                Diagnostic::error(
                    codes::INVALID_PARAMS,
                    format!("Couldn't read the engine installation summary: {error}"),
                )
                .into()
            })
        });
        match result {
            Ok(plan) => {
                self.releases.plans.insert(version, plan);
            }
            Err(error) => {
                if !visible {
                    return;
                }
                let id = self.notice("Couldn't check installation space".into(), Some(&error));
                if let Some(notice) = self.notices.iter_mut().find(|notice| notice.id == id) {
                    notice.retry = Some(Box::new(Intent::Dialog(retry)));
                }
            }
        }
    }
    /// An explicit opt-in can download, but never produces a restart effect.
    pub(super) fn prepare_background_update(&mut self) {
        if self.prefs.hub_update_downloads != DownloadPolicy::Background
            || !self.releases.can_self_update
        {
            return;
        }
        let Some(release) = &self.releases.hub else {
            return;
        };
        let identity = identity(release);
        if self.prefs.skipped_update.as_deref() == Some(identity.as_str())
            || self
                .releases
                .jobs
                .iter()
                .any(|job| job.kind == "hub" && matches_release(job, release))
            || !self.releases.attempted_updates.insert(identity)
        {
            return;
        }
        let version = release.version.clone();
        self.start_transfer("hub", version, "hub.update.prepare");
    }
    pub(super) fn release_call(&mut self, method: &'static str, params: Value) {
        let call = Call::Release { method, params };
        if !self.calls.contains(&call) {
            self.send(call);
        }
    }
    pub(super) fn check_updates(&mut self) {
        self.releases.checking = true;
        self.release_call("hub.releases.check", json!({}));
        self.refresh_releases();
    }
    pub(super) fn refresh_releases(&mut self) {
        self.release_call(
            "hub.engines.available",
            json!({"channel":self.releases.channel}),
        );
        self.release_call("hub.updates", json!({}));
        self.release_call("hub.downloads", json!({}));
    }
    pub(super) fn install_engine(&mut self, version: String) {
        if self.releases.checking {
            self.releases.pending_installs.insert(version);
            return;
        }
        self.start_transfer("engine", version, "hub.engine.install");
    }
    pub(super) fn restart_update(&mut self) {
        if let Some(result) = self.releases.prepared() {
            let archive = result["archive"].as_str().map(PathBuf::from);
            let manifest = result["manifest"].as_str();
            let signature = result["signature"].as_str();
            if let (Some(archive), Some(manifest), Some(signature)) = (archive, manifest, signature)
            {
                self.effects.push(Effect::ApplyUpdate {
                    archive,
                    manifest: manifest.into(),
                    signature: signature.into(),
                });
            }
        }
    }
    pub(super) fn release_reply(&mut self, method: &str, result: Result<Value, RpcError>) {
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.notice("Release operation failed".into(), Some(&error));
                return;
            }
        };
        match method {
            "hub.engines.available" | "hub.updates" => {
                if let Some(root) = value["installRoot"].as_str() {
                    self.releases.install_root = Some(root.into());
                }
                let releases: Vec<Manifest> =
                    serde_json::from_value(value["releases"].clone()).unwrap_or_default();
                if method == "hub.engines.available" {
                    self.releases.available = releases;
                } else {
                    let current = crate::build_identity();
                    self.releases.hub = releases
                        .into_iter()
                        .filter(|r| {
                            ara_update::build::newer(
                                &r.version,
                                r.build.as_ref(),
                                env!("CARGO_PKG_VERSION"),
                                current.as_ref(),
                            )
                        })
                        .max_by_key(|r| {
                            (
                                ara_update::version(&r.version).ok(),
                                r.build.as_ref().map(|b| b.generation),
                            )
                        });
                    if let Ok(channel) = serde_json::from_value(value["channel"].clone()) {
                        if channel != self.releases.channel {
                            self.release_call("hub.engines.available", json!({"channel":channel}));
                        }
                        self.releases.channel = channel;
                    }
                    if let Some(repo) = value["repo"].as_str() {
                        self.releases.repo = repo.into();
                    }
                    self.prepare_background_update();
                }
                self.releases.checking = value["checking"] == true;
                if !self.releases.checking && method == "hub.engines.available" {
                    let pending = std::mem::take(&mut self.releases.pending_installs);
                    for version in pending {
                        self.install_engine(version);
                    }
                }
                self.releases.status = match value["status"]["state"].as_str() {
                    Some("fresh") => "Release list is up to date".into(),
                    Some("offline") => "Offline — showing saved releases".into(),
                    Some("rateLimited") => format!(
                        "Release service is busy — showing saved releases. Retry after {}",
                        value["status"]["retryAfter"]
                            .as_str()
                            .unwrap_or("the limit resets")
                    ),
                    Some("error") => format!(
                        "{} {}",
                        value["status"]["diagnostic"]["code"],
                        value["status"]["diagnostic"]["message"]
                            .as_str()
                            .unwrap_or("Release check failed")
                    ),
                    _ => "Checking for releases…".into(),
                };
            }
            "hub.downloads" => {
                let jobs: Vec<Download> =
                    serde_json::from_value(value["jobs"].clone()).unwrap_or_default();
                for job in &jobs {
                    if job.kind == "uninstall-preflight" {
                        self.receive_removal(job);
                        continue;
                    }
                    if matches!(job.kind.as_str(), "engine" | "engine-current" | "repair")
                        && job.state == "cancelled"
                    {
                        if self.engine_layout_owns_install(&job.id) {
                            self.engine_layout_cancelled(&job.version);
                        }
                        self.releases.open_after.remove(&job.version);
                    }
                    let was_finished = self
                        .releases
                        .jobs
                        .iter()
                        .any(|old| old.id == job.id && old.state == job.state);
                    if job.state == "complete"
                        && !was_finished
                        && matches!(
                            job.kind.as_str(),
                            "engine" | "engine-current" | "repair" | "uninstall"
                        )
                    {
                        self.refresh([List::Engines]);
                        if matches!(job.kind.as_str(), "engine" | "engine-current" | "repair") {
                            self.engine_layout_installed(&job.version, &job.id);
                        }
                        if job.kind == "uninstall" {
                            self.notice(format!("Uninstalled Ara {}", job.version), None);
                            continue;
                        }
                        if job.kind == "repair" {
                            self.notice(format!("Repaired Ara {}", job.version), None);
                            continue;
                        }
                        if let Some(projects) = self.releases.open_after.remove(&job.version) {
                            for (project, headless) in projects {
                                self.lifecycle(
                                    project,
                                    if headless {
                                        Busy::Starting
                                    } else {
                                        Busy::Opening
                                    },
                                );
                            }
                        }
                    }
                    if let Some(error) = &job.error
                        && !was_finished
                    {
                        let retry = if self.engine_layout_owns_install(&job.id) {
                            self.engine_layout_transfer_failed(&job.version)
                        } else {
                            None
                        };
                        let notice = self.notice(
                            format!(
                                "Couldn't {} Ara {}",
                                if job.kind == "uninstall" {
                                    "uninstall"
                                } else if job.kind == "hub" {
                                    "update"
                                } else if job.kind == "repair" {
                                    "repair"
                                } else {
                                    "install"
                                },
                                job.version
                            ),
                            Some(&error.clone().into()),
                        );
                        if let Some(retry) = retry
                            && let Some(notice) =
                                self.notices.iter_mut().find(|entry| entry.id == notice)
                        {
                            notice.retry = Some(Box::new(retry));
                        }
                        self.releases.open_after.remove(&job.version);
                    }
                }
                self.releases.jobs = jobs;
            }
            "hub.engine.install"
            | "hub.engine.installCurrent"
            | "hub.engine.repair"
            | "hub.update.prepare"
            | "hub.engine.uninstall.preflight"
            | "hub.engine.uninstall" => {
                if let Ok(job) = serde_json::from_value::<Download>(value) {
                    if method == "hub.engine.installCurrent" {
                        self.engine_layout_install_started(&job);
                    }
                    let key = (job.kind.clone(), job.version.clone());
                    self.releases.requests.remove(&key);
                    if self.releases.cancelling_requests.remove(&key) {
                        self.release_call("hub.download.cancel", json!({"job":job.id}));
                    }
                    self.receive_removal(&job);
                    self.releases.jobs.retain(|j| j.id != job.id);
                    self.releases.jobs.push(job);
                }
            }
            "hub.releases.configure" => self.refresh_releases(),
            _ => {}
        }
        self.restart_removal_checks();
    }
    /// Native-shell errors use the same catalogued dialog path as service errors.
    pub fn update_error(&mut self, message: String) {
        self.notice(
            "Couldn't restart Ara Hub".into(),
            Some(
                &ara_update::Error {
                    code: ara_update::IO,
                    message,
                }
                .diagnostic()
                .into(),
            ),
        );
    }
}

#[cfg(test)]
mod build_tests {
    use super::*;
    #[test]
    fn newer_same_version_hub_does_not_reuse_an_older_prepared_download() {
        let mut hub = Hub::new(Default::default(), "target/hub-build-model".into(), None);
        hub.releases.can_self_update = true;
        let current = crate::build_identity();
        let generation = current.as_ref().map_or(0, |b| b.generation) + 1;
        let commit = if current.as_ref().is_some_and(|b| b.commit == "a".repeat(40)) {
            "b"
        } else {
            "a"
        }
        .repeat(40);
        let version = env!("CARGO_PKG_VERSION");
        hub.releases.jobs.push(serde_json::from_value(json!({"id":format!("hub-{version}-old"),"kind":"hub","version":version,"state":"complete","result":{"archive":"old.zip"}})).expect("old download"));
        hub.release_reply("hub.updates",Ok(json!({"releases":[{
            "format":"ara.release/1","kind":"hub","version":version,"build":{"commit":commit,"generation":generation},"channel":"preview",
            "minHubVersion":version,"notes":"New preview","platform":"windows-x64","assets":[]
        }],"checking":false,"channel":"preview","repo":"Ara-Engine/ara"})));
        assert_eq!(
            hub.releases
                .hub
                .as_ref()
                .and_then(|r| r.build.as_ref())
                .map(|b| b.generation),
            Some(generation)
        );
        assert!(
            hub.releases.prepared().is_none(),
            "old build archive is not ready for this update"
        );
        assert!(!hub.take_calls().iter().any(|call| matches!(
            call,
            Call::Release {
                method: "hub.update.prepare",
                ..
            }
        )));
    }
}
