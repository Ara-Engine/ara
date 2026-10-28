//! Release I/O workers never hold the serialized registry authority.
mod removal_preflight;
mod repair;
mod uninstall;
use super::*;
use ara_update::{
    Cancel, Channel, Feed, FeedConfig, FeedSnapshot, Kind, Progress, VerifiedRelease,
};
use serde::Serialize;
use std::sync::mpsc::{self, Receiver, Sender};

/// Release dependencies overridable by integration tests with a local fake server.
pub struct UpdateConfig {
    /// Repository/channel/trust configuration.
    pub feed: FeedConfig,
    /// Hub-owned side-by-side engine installations.
    pub engines: PathBuf,
}
impl UpdateConfig {
    pub(super) fn user(directory: &Path) -> Result<Self, RpcError> {
        Ok(Self {
            feed: FeedConfig::load(directory).map_err(rpc)?,
            engines: std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| directory.to_path_buf())
                .join("Ara/Engines"),
        })
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: String,
    kind: String,
    version: String,
    state: String,
    phase: String,
    cancellable: bool,
    disk: Option<ara_update::DiskSpace>,
    progress: Progress,
    result: Option<Value>,
    error: Option<ara_diag::Diagnostic>,
    #[serde(skip)]
    cancel: Cancel,
    #[serde(skip)]
    proof: Option<(String, String)>,
    #[serde(skip)]
    repair_source: Option<EngineBuild>,
}
enum Message {
    Feed(Result<FeedSnapshot, ara_update::Error>),
    Progress(String, Progress),
    Phase(String, &'static str),
    UninstallPrepared(String, Result<PathBuf, ara_update::Error>),
    Uninstalled(String, Result<PathBuf, ara_update::Error>),
    UninstallInspected(
        String,
        Result<removal_preflight::Inspection, ara_update::Error>,
    ),
    RepairReady(String, Result<PathBuf, ara_update::Error>),
    RepairCleaned(String, Result<(), ara_update::Error>),
    Done(String, Result<PathBuf, ara_update::Error>),
}
pub(super) struct Updates {
    config: UpdateConfig,
    cache: PathBuf,
    staging: PathBuf,
    releases: Vec<VerifiedRelease>,
    status: Value,
    checking: bool,
    checked: Option<Instant>,
    jobs: BTreeMap<String, Job>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}
impl Updates {
    pub(super) fn busy(&self) -> bool {
        self.jobs.values().any(|job| job.state == "running")
    }
    pub(super) fn new(config: UpdateConfig, directory: &Path) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            config,
            cache: directory.join("release-cache"),
            staging: directory.join("downloads"),
            releases: Vec::new(),
            status: json!({"state":"unchecked"}),
            checking: false,
            checked: None,
            jobs: BTreeMap::new(),
            tx,
            rx,
        }
    }
    fn check(&mut self) -> Result<(), RpcError> {
        if self.checking {
            return Ok(());
        }
        let config = self.config.feed.clone();
        let cache = self.cache.clone();
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("ara-release-feed".into())
            .spawn(move || {
                background_priority();
                let result = Feed::new(config, &cache).and_then(|feed| feed.check());
                let _ = tx.send(Message::Feed(result));
            })
            .map_err(io_error)?;
        self.checking = true;
        self.checked = Some(Instant::now());
        Ok(())
    }
}
fn rpc(error: ara_update::Error) -> RpcError {
    error.diagnostic().into()
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    version: Option<String>,
    channel: Option<Channel>,
    job: Option<String>,
    repo: Option<String>,
    hub_version: Option<String>,
}

impl Hub {
    /// Apply completed workers on the registry thread, then notify every subscribed client.
    pub(super) fn poll_updates(&mut self) {
        let messages: Vec<_> = self.updates.rx.try_iter().collect();
        for message in messages {
            match message {
                Message::Feed(result) => {
                    self.updates.checking = false;
                    match result {
                        Ok(feed) => {
                            self.updates.releases = feed.releases;
                            self.updates.status = json!(feed.status);
                        }
                        Err(error) => {
                            self.updates.status =
                                json!({"state":"error","diagnostic":error.diagnostic()})
                        }
                    }
                    self.changed("releases", None);
                }
                Message::Progress(id, progress) => {
                    if let Some(job) = self.updates.jobs.get_mut(&id) {
                        job.progress = progress;
                    }
                    self.changed("progress", None);
                }
                Message::Phase(id, phase) => {
                    if let Some(job) = self.updates.jobs.get_mut(&id) {
                        job.phase = phase.into();
                    }
                    self.changed("progress", None);
                }
                Message::UninstallPrepared(id, result) => self.commit_uninstall(id, result),
                Message::Uninstalled(id, result) => self.finish_uninstall(id, result),
                Message::UninstallInspected(id, result) => {
                    self.finish_uninstall_preflight(id, result)
                }
                Message::RepairReady(id, result) => self.commit_repair(id, result),
                Message::RepairCleaned(id, result) => self.finish_repair(id, result),
                Message::Done(id, result) => {
                    let Some(mut job) = self.updates.jobs.remove(&id) else {
                        continue;
                    };
                    let result = result.and_then(|path| {
                        if let Err(error) = job.cancel.check() {
                            if job.kind == "engine" {
                                if !self.registry.engines.iter().any(|engine| same_executable(engine, &path)) {
                                    let _ = fs::remove_dir_all(&path);
                                }
                            } else {
                                let _ = fs::remove_file(&path);
                            }
                            return Err(error);
                        }
                        if job.kind == "engine" {
                            if let Some(existing) = self.registry.engines.iter().find(|engine| engine.version == job.version) {
                                if same_executable(existing, &path) {
                                    return Ok(json!({"engine":existing}));
                                }
                                let _ = fs::remove_dir_all(&path);
                                return Err(ara_update::Error { code: ara_update::IO, message: "this version was registered while the download was running; the registered build was preserved".into() });
                            }
                            let engine = EngineBuild {
                                version: job.version.clone(),
                                executable: path.join("ara.exe"),
                                editor: Some(path.join("ara-editor.exe")),
                            };
                            let previous = self.registry.engines.clone();
                            self.registry.engines.retain(|e| e.version != job.version);
                            self.registry.engines.push(engine.clone());
                            if let Err(error) = self.registry.save(&self.config.directory) {
                                self.registry.engines = previous;
                                let _ = fs::remove_dir_all(&path);
                                return Err(ara_update::Error {
                                    code: ara_update::IO,
                                    message: error.message,
                                });
                            }
                            Ok(json!({"engine":engine}))
                        } else {
                            let (manifest, signature) =
                                job.proof.as_ref().ok_or_else(|| ara_update::Error {
                                    code: ara_update::MANIFEST,
                                    message: "prepared Hub release has no signature proof".into(),
                                })?;
                            Ok(json!({"archive":path,"manifest":manifest,"signature":signature}))
                        }
                    });
                    match result {
                        Ok(result) => {
                            job.state = "complete".into();
                            job.phase = "complete".into();
                            job.cancellable = false;
                            job.result = Some(result);
                        }
                        Err(error) => {
                            job.state = if error.code == ara_update::CANCELLED {
                                "cancelled"
                            } else {
                                "failed"
                            }
                            .into();
                            job.error = Some(error.diagnostic());
                            job.phase = job.state.clone();
                            job.cancellable = false;
                        }
                    }
                    self.updates.jobs.insert(id, job);
                    self.changed("engine", None);
                    self.changed("progress", None);
                }
            }
        }
    }
    pub(super) fn update_call(&mut self, method: &str, value: Value) -> Result<Value, RpcError> {
        let p: Request = parse(value)?;
        match method {
            "hub.engines.available" | "hub.updates" => {
                if self
                    .updates
                    .checked
                    .is_none_or(|checked| checked.elapsed() >= Duration::from_secs(4 * 3600))
                {
                    self.updates.check()?;
                }
                let kind = if method == "hub.engines.available" {
                    Kind::Engine
                } else {
                    Kind::Hub
                };
                let channel = p.channel.unwrap_or(self.updates.config.feed.channel);
                let releases: Vec<_> = self
                    .updates
                    .releases
                    .iter()
                    .filter(|r| {
                        r.manifest().kind == kind
                            && (channel == Channel::Preview
                                || r.manifest().channel == Channel::Stable)
                    })
                    .map(|r| r.manifest())
                    .collect();
                Ok(
                    json!({"releases":releases,"status":self.updates.status,"checking":self.updates.checking,"repo":self.updates.config.feed.repo,"channel":self.updates.config.feed.channel,"installRoot":self.updates.config.engines}),
                )
            }
            "hub.releases.check" => {
                self.updates.check()?;
                Ok(json!({"checking":true}))
            }
            "hub.releases.configure" => {
                if self.updates.checking || self.updates.jobs.values().any(|j| j.state == "running")
                {
                    return Err(invalid(
                        "wait for active release work before changing the feed",
                    ));
                }
                let mut feed = self.updates.config.feed.clone();
                if let Some(repo) = p.repo {
                    feed.repo = repo;
                }
                if let Some(channel) = p.channel {
                    feed.channel = channel;
                }
                feed.validate().map_err(rpc)?;
                ara_update::write_json(&self.config.directory.join("update.json"), &feed)
                    .map_err(rpc)?;
                if let Ok(repo) = std::env::var("ARA_RELEASE_REPO") {
                    feed.repo = repo;
                }
                self.updates.config.feed = feed;
                self.updates.releases.clear();
                self.updates.check()?;
                Ok(json!({"checking":true}))
            }
            "hub.downloads" => Ok(json!({"jobs":self.updates.jobs.values().collect::<Vec<_>>()})),
            "hub.download.cancel" => {
                let id = required(p.job, "job")?;
                let job = self
                    .updates
                    .jobs
                    .get(&id)
                    .ok_or_else(|| invalid("unknown download job"))?;
                if job.state == "complete" || (job.state == "running" && !job.cancellable) {
                    return Err(invalid(
                        "operation has committed; it can no longer be cancelled",
                    ));
                }
                job.cancel.cancel();
                if job.kind != "uninstall-preflight"
                    && (job.state == "failed" || job.state == "cancelled")
                {
                    let stage = self.updates.staging.join(&id);
                    if stage.exists() {
                        fs::remove_dir_all(stage).map_err(io_error)?;
                    }
                }
                Ok(json!({"cancelling":id}))
            }
            "hub.engine.preflight" => self.engine_preflight(required(p.version, "version")?),
            "hub.engine.uninstall.preflight" => {
                self.begin_uninstall_preflight(required(p.version, "version")?)
            }
            "hub.engine.install" | "hub.engine.repair" | "hub.update.prepare" => {
                if self.updates.status["state"] == "error"
                    && let Ok(diagnostic) = serde_json::from_value::<ara_diag::Diagnostic>(
                        self.updates.status["diagnostic"].clone(),
                    )
                {
                    return Err(diagnostic.into());
                }
                let version = required(p.version, "version")?;
                ara_update::version(&version).map_err(rpc)?;
                let repair = method == "hub.engine.repair";
                let kind = if method != "hub.update.prepare" {
                    Kind::Engine
                } else {
                    Kind::Hub
                };
                if kind == Kind::Engine
                    && !repair
                    && self.registry.engines.iter().any(|e| e.version == version)
                {
                    return Err(invalid("this engine version is already registered"));
                }
                if kind == Kind::Engine && self.engine_removal_running(&version) {
                    return Err(invalid(
                        "wait for this version's uninstall to finish before installing it again",
                    ));
                }
                let repair_source = if repair {
                    Some(self.validate_repair(&version)?)
                } else {
                    None
                };
                let release = self
                    .updates
                    .releases
                    .iter()
                    .find(|r| r.manifest().kind == kind && r.manifest().version == version)
                    .cloned()
                    .ok_or_else(|| {
                        invalid(
                            "version is not in the authenticated feed; check for releases first",
                        )
                    })?;
                let hub_version = p
                    .hub_version
                    .as_deref()
                    .unwrap_or(env!("CARGO_PKG_VERSION"));
                if kind == Kind::Engine
                    && ara_update::version(hub_version).map_err(rpc)?
                        < ara_update::version(&release.manifest().min_hub_version).map_err(rpc)?
                {
                    return Err(invalid(format!(
                        "update Ara Hub to {} before installing this version",
                        release.manifest().min_hub_version
                    )));
                }
                let id = if repair {
                    format!("repair-{version}")
                } else {
                    match release
                        .manifest()
                        .build
                        .as_ref()
                        .filter(|_| kind == Kind::Hub)
                    {
                        Some(build) => format!("hub-{version}-{}", build.commit),
                        None => format!("{}-{version}", kind.name()),
                    }
                };
                if let Some(job) = self.updates.jobs.get(&id)
                    && job.state == "running"
                {
                    return Ok(json!(job));
                }
                let stage = self.updates.staging.join(&id);
                let asset = release.archive().map_err(rpc)?;
                let resumed = fs::metadata(stage.join(format!("{}.part", asset.sha256)))
                    .map_or(0, |metadata| metadata.len().min(asset.size));
                let disk = ara_update::check_space(&stage, asset.size - resumed).map_err(rpc)?;
                let root = self.updates.config.engines.clone();
                let config = self.updates.config.feed.clone();
                let cache = self.updates.cache.clone();
                let cancel = Cancel::default();
                let flag = cancel.clone();
                let tx = self.updates.tx.clone();
                let worker_id = id.clone();
                let job = Job {
                    id: id.clone(),
                    kind: if repair { "repair" } else { kind.name() }.into(),
                    version: version.clone(),
                    state: "running".into(),
                    phase: "downloading".into(),
                    cancellable: true,
                    disk: Some(disk),
                    progress: Progress {
                        received: 0,
                        total: release.archive().map_err(rpc)?.size,
                    },
                    result: None,
                    error: None,
                    cancel,
                    proof: (kind == Kind::Hub).then(|| {
                        let (manifest, signature) = release.proof();
                        (manifest.to_owned(), signature.to_owned())
                    }),
                    repair_source,
                };
                std::thread::Builder::new()
                    .name(format!("ara-install-{id}"))
                    .spawn(move || {
                        background_priority();
                        let result = (|| {
                            let feed = Feed::new(config, &cache)?;
                            let mut last = Instant::now() - Duration::from_secs(1);
                            let archive = ara_update::download_with_phase(
                                &feed,
                                &release,
                                &stage,
                                &flag,
                                |progress| {
                                    if last.elapsed() >= Duration::from_millis(150)
                                        || progress.received == progress.total
                                    {
                                        let _ =
                                            tx.send(Message::Progress(worker_id.clone(), progress));
                                        last = Instant::now();
                                    }
                                },
                                |phase| {
                                    let phase = match phase {
                                        ara_update::DownloadPhase::Downloading => "downloading",
                                        ara_update::DownloadPhase::Verifying => "verifying",
                                    };
                                    let _ = tx.send(Message::Phase(worker_id.clone(), phase));
                                },
                            )?;
                            if kind == Kind::Hub {
                                return Ok(archive);
                            }
                            let _ = tx.send(Message::Phase(worker_id.clone(), "extracting"));
                            if repair {
                                return repair::prepare(&root, &version, &archive, &flag);
                            }
                            install_engine(&root, &version, &archive, &flag, || {
                                let _ = tx.send(Message::Phase(worker_id.clone(), "installing"));
                            })
                        })();
                        if result
                            .as_ref()
                            .is_err_and(|e: &ara_update::Error| e.code == ara_update::CANCELLED)
                            || (result.is_ok() && kind == Kind::Engine)
                        {
                            let _ = fs::remove_dir_all(&stage);
                        }
                        let _ = tx.send(if repair {
                            Message::RepairReady(worker_id, result)
                        } else {
                            Message::Done(worker_id, result)
                        });
                    })
                    .map_err(io_error)?;
                self.updates.jobs.insert(id, job.clone());
                self.changed("progress", None);
                Ok(json!(job))
            }
            "hub.engine.uninstall" => self.begin_uninstall(required(p.version, "version")?),
            _ => Err(invalid("unknown release method")),
        }
    }
}
fn install_engine(
    root: &Path,
    version: &str,
    archive: &Path,
    cancel: &Cancel,
    installing: impl FnOnce(),
) -> Result<PathBuf, ara_update::Error> {
    fs::create_dir_all(root)?;
    let destination = root.join(version);
    let staging = root.join(format!(".install-{version}"));
    if destination.exists() || staging.exists() {
        return Err(ara_update::Error {
            code: ara_update::IO,
            message: "engine destination or unfinished staging directory already exists".into(),
        });
    }
    ara_update::extract(archive, &staging, cancel)?;
    let result = (|| {
        validate_payload(&staging, version)?;
        ara_update::write_json(
            &staging.join(".ara-managed.json"),
            &json!({"version":version}),
        )?;
        cancel.check()?;
        installing();
        fs::rename(&staging, &destination)?;
        Ok(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn validate_payload(staging: &Path, version: &str) -> Result<(), ara_update::Error> {
    let marker: Value = serde_json::from_slice(&fs::read(staging.join("engine.json"))?)?;
    if marker["format"] != "ara.engine/1"
        || marker["version"] != version
        || !staging.join("ara.exe").is_file()
        || !staging.join("ara-editor.exe").is_file()
        || !staging.join("ara-player.exe").is_file()
    {
        return Err(ara_update::Error {
            code: ara_update::MANIFEST,
            message: "engine archive is incomplete or has a different version".into(),
        });
    }
    Ok(())
}

fn background_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: GetCurrentThread returns a valid pseudo-handle; this changes only this I/O worker's priority.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
}

// A build can be registered while the worker finishes filesystem cleanup. Preserve
// that independent registration, including canonical Windows path aliases.
fn same_executable(engine: &EngineBuild, directory: &Path) -> bool {
    let target = directory.join("ara.exe");
    engine.executable == target
        || fs::canonicalize(&engine.executable)
            .ok()
            .zip(fs::canonicalize(target).ok())
            .is_some_and(|(left, right)| left == right)
}
