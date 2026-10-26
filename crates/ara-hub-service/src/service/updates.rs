//! Release I/O workers never hold the serialized registry authority.
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
    progress: Progress,
    result: Option<Value>,
    error: Option<ara_diag::Diagnostic>,
    #[serde(skip)]
    cancel: Cancel,
    #[serde(skip)]
    proof: Option<(String, String)>,
}
enum Message {
    Feed(Result<FeedSnapshot, ara_update::Error>),
    Progress(String, Progress),
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
                    json!({"releases":releases,"status":self.updates.status,"checking":self.updates.checking,"repo":self.updates.config.feed.repo,"channel":self.updates.config.feed.channel}),
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
                if job.state == "complete" {
                    return Err(invalid("download already completed"));
                }
                job.cancel.cancel();
                if job.state == "failed" || job.state == "cancelled" {
                    let stage = self.updates.staging.join(&id);
                    if stage.exists() {
                        fs::remove_dir_all(stage).map_err(io_error)?;
                    }
                }
                Ok(json!({"cancelling":id}))
            }
            "hub.engine.install" | "hub.update.prepare" => {
                if self.updates.status["state"] == "error"
                    && let Ok(diagnostic) = serde_json::from_value::<ara_diag::Diagnostic>(
                        self.updates.status["diagnostic"].clone(),
                    )
                {
                    return Err(diagnostic.into());
                }
                let version = required(p.version, "version")?;
                ara_update::version(&version).map_err(rpc)?;
                let kind = if method == "hub.engine.install" {
                    Kind::Engine
                } else {
                    Kind::Hub
                };
                if kind == Kind::Engine
                    && self.registry.engines.iter().any(|e| e.version == version)
                {
                    return Err(invalid("this engine version is already registered"));
                }
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
                let id = match release
                    .manifest()
                    .build
                    .as_ref()
                    .filter(|_| kind == Kind::Hub)
                {
                    Some(build) => format!("hub-{version}-{}", build.commit),
                    None => format!("{}-{version}", kind.name()),
                };
                if let Some(job) = self.updates.jobs.get(&id)
                    && job.state == "running"
                {
                    return Ok(json!(job));
                }
                let stage = self.updates.staging.join(&id);
                let root = self.updates.config.engines.clone();
                let config = self.updates.config.feed.clone();
                let cache = self.updates.cache.clone();
                let cancel = Cancel::default();
                let flag = cancel.clone();
                let tx = self.updates.tx.clone();
                let worker_id = id.clone();
                let job = Job {
                    id: id.clone(),
                    kind: kind.name().into(),
                    version: version.clone(),
                    state: "running".into(),
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
                };
                std::thread::Builder::new()
                    .name(format!("ara-install-{id}"))
                    .spawn(move || {
                        background_priority();
                        let result = (|| {
                            let feed = Feed::new(config, &cache)?;
                            let mut last = Instant::now() - Duration::from_secs(1);
                            let archive =
                                ara_update::download(&feed, &release, &stage, &flag, |progress| {
                                    if last.elapsed() >= Duration::from_millis(150)
                                        || progress.received == progress.total
                                    {
                                        let _ =
                                            tx.send(Message::Progress(worker_id.clone(), progress));
                                        last = Instant::now();
                                    }
                                })?;
                            if kind == Kind::Hub {
                                return Ok(archive);
                            }
                            install_engine(&root, &version, &archive, &flag)
                        })();
                        if result
                            .as_ref()
                            .is_err_and(|e: &ara_update::Error| e.code == ara_update::CANCELLED)
                            || (result.is_ok() && kind == Kind::Engine)
                        {
                            let _ = fs::remove_dir_all(&stage);
                        }
                        let _ = tx.send(Message::Done(worker_id, result));
                    })
                    .map_err(io_error)?;
                self.updates.jobs.insert(id, job.clone());
                self.changed("progress", None);
                Ok(json!(job))
            }
            "hub.engine.uninstall" => {
                let version = required(p.version, "version")?;
                ara_update::version(&version).map_err(rpc)?;
                let engine = self.registry.engine(&version)?.clone();
                if self
                    .registry
                    .projects
                    .iter()
                    .any(|p| p.engine_version == version && self.status_of(p)["state"] == "open")
                {
                    return Err(invalid(
                        "close projects using this engine before uninstalling",
                    ));
                }
                let root = self.updates.config.engines.join(&version);
                if self.config.engine.executable.starts_with(&root) {
                    return Err(invalid(
                        "this engine hosts the running Hub service; restart the service from the standalone Hub before uninstalling",
                    ));
                }
                if engine.executable != root.join("ara.exe")
                    || !root.join(".ara-managed.json").is_file()
                    || fs::symlink_metadata(&root)
                        .map_err(io_error)?
                        .file_type()
                        .is_symlink()
                {
                    return Err(invalid(
                        "only Hub-managed installations can be uninstalled; remove registration for external builds",
                    ));
                }
                let trash = root.with_file_name(format!(".uninstall-{version}"));
                fs::rename(&root, &trash).map_err(io_error)?;
                let previous = self.registry.engines.clone();
                self.registry.engines.retain(|e| e.version != version);
                if let Err(error) = self.registry.save(&self.config.directory) {
                    self.registry.engines = previous;
                    fs::rename(&trash, &root).map_err(io_error)?;
                    return Err(error);
                }
                fs::remove_dir_all(trash).map_err(io_error)?;
                self.changed("engine", None);
                Ok(json!({"removed":version}))
            }
            _ => Err(invalid("unknown release method")),
        }
    }
}
fn install_engine(
    root: &Path,
    version: &str,
    archive: &Path,
    cancel: &Cancel,
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
        ara_update::write_json(
            &staging.join(".ara-managed.json"),
            &json!({"version":version}),
        )?;
        cancel.check()?;
        fs::rename(&staging, &destination)?;
        Ok(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
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
