//! Release view state uses the service's background jobs, including install-and-open.
use super::*;
use ara_update::{Channel, Manifest};
use serde_json::json;

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
    /// Streamed bytes and signed total.
    pub progress: Option<ara_update::Progress>,
    /// Installed build or authenticated Hub staging result.
    pub result: Option<Value>,
    /// Catalogued failure.
    pub error: Option<Diagnostic>,
}
/// Data rendered by Installs and Settings.
#[derive(Debug)]
pub struct Releases {
    /// Standalone installs permit automatic preparation of a Hub update.
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
    /// Feed freshness or diagnostic message.
    pub status: String,
    /// A check is in flight.
    pub checking: bool,
    pub(super) open_after: BTreeMap<String, BTreeMap<String, bool>>,
    pending_installs: BTreeSet<String>,
}
impl Default for Releases {
    fn default() -> Self {
        Self {
            can_self_update: false,
            available: Vec::new(),
            hub: None,
            jobs: Vec::new(),
            channel: Channel::Stable,
            repo: "Snowy7/ara".into(),
            status: "Check for published versions".into(),
            checking: false,
            open_after: BTreeMap::new(),
            pending_installs: BTreeSet::new(),
        }
    }
}
impl Releases {
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
fn matches_release(job: &Download, release: &Manifest) -> bool {
    let id = match &release.build {
        Some(build) => format!("hub-{}-{}", release.version, build.commit),
        None => format!("hub-{}", release.version),
    };
    job.id == id
}

impl Hub {
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
        self.release_call(
            "hub.engine.install",
            json!({"version":version,"hubVersion":env!("CARGO_PKG_VERSION")}),
        );
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
                if method == "hub.engine.install" {
                    self.releases.open_after.clear();
                }
                self.notice("Release operation failed".into(), Some(&error));
                return;
            }
        };
        match method {
            "hub.engines.available" | "hub.updates" => {
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
                    if self.releases.can_self_update
                        && let Some(hub) = &self.releases.hub
                        && !self
                            .releases
                            .jobs
                            .iter()
                            .any(|j| j.kind == "hub" && matches_release(j, hub))
                    {
                        self.release_call(
                            "hub.update.prepare",
                            json!({"version":hub.version,"hubVersion":env!("CARGO_PKG_VERSION")}),
                        );
                    }
                }
                self.releases.checking = value["checking"] == true;
                if !self.releases.checking && method == "hub.engines.available" {
                    let pending = std::mem::take(&mut self.releases.pending_installs);
                    for version in pending {
                        self.install_engine(version);
                    }
                }
                self.releases.status = match value["status"]["state"].as_str() {
                    Some("fresh") => "Up to date with GitHub Releases".into(),
                    Some("offline") => "Offline — showing verified cached releases".into(),
                    Some("rateLimited") => format!(
                        "GitHub rate limit — showing cached releases. Retry after {}",
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
                    _ => "Checking GitHub Releases…".into(),
                };
            }
            "hub.downloads" => {
                let jobs: Vec<Download> =
                    serde_json::from_value(value["jobs"].clone()).unwrap_or_default();
                for job in &jobs {
                    let was_finished = self
                        .releases
                        .jobs
                        .iter()
                        .any(|old| old.id == job.id && old.state == job.state);
                    if job.state == "complete" && !was_finished && job.kind == "engine" {
                        self.refresh([List::Engines]);
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
                        self.notice(
                            format!("Couldn't install Ara {}", job.version),
                            Some(&error.clone().into()),
                        );
                        self.releases.open_after.remove(&job.version);
                    }
                }
                self.releases.jobs = jobs;
            }
            "hub.engine.install" | "hub.update.prepare" => {
                if let Ok(job) = serde_json::from_value::<Download>(value) {
                    self.releases.jobs.retain(|j| j.id != job.id);
                    self.releases.jobs.push(job);
                }
            }
            "hub.engine.uninstall" => self.refresh([List::Engines]),
            "hub.releases.configure" => self.refresh_releases(),
            _ => {}
        }
    }
    /// Native-shell errors use the same catalogue/toast path as service errors.
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
        }],"checking":false,"channel":"preview","repo":"Snowy7/ara"})));
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
        assert!(hub.take_calls().iter().any(|call| matches!(
            call,
            Call::Release {
                method: "hub.update.prepare",
                ..
            }
        )));
    }
}
