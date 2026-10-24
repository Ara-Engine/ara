//! Serialized Hub registry authority.
mod background;
mod create;
mod lifecycle;
mod transport;
mod updates;
use crate::{
    DirectoryTemplates, EngineBuild, ProjectEntry, Registry, TemplateSource, invalid, io_error,
    now, registry::write_json, templates::Layered,
};
use ara_proto::{ConnId, Endpoint, Incoming, RpcError, jsonrpc};
use ara_rpc::{Client, DaemonInfo, Handler, ServeOptions};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
pub use transport::{connect_or_start, serve};
pub use updates::UpdateConfig;

/// Hub construction dependencies. Templates are replaceable without changing the wire API.
pub struct HubConfig {
    /// User configuration and discovery directory.
    pub directory: PathBuf,
    /// The running distribution, registered on first startup.
    pub engine: EngineBuild,
    /// Project template source.
    pub templates: Box<dyn TemplateSource>,
}
impl HubConfig {
    /// Standard user configuration for the current `ara` executable.
    pub fn user(executable: PathBuf) -> io::Result<Self> {
        let directory = crate::config_dir()?;
        let editor = executable.with_file_name(if cfg!(windows) {
            "ara-editor.exe"
        } else {
            "ara-editor"
        });
        Ok(Self {
            templates: Box::new(DirectoryTemplates(
                executable
                    .parent()
                    .map(|dir| dir.join("templates"))
                    .filter(|dir| dir.is_dir())
                    .unwrap_or_else(|| directory.join("templates")),
            )),
            directory,
            engine: EngineBuild {
                version: crate::ALPHA_VERSION.into(),
                executable,
                editor: editor.is_file().then_some(editor),
            },
        })
    }
}

/// User-level project authority. Every engine daemon remains in its own process.
pub struct Hub {
    updates: updates::Updates,
    config: HubConfig,
    registry: Registry,
    children: BTreeMap<String, Child>,
    routes: BTreeMap<(ConnId, String), Client>,
    subscribers: BTreeSet<ConnId>,
    events: Vec<(ConnId, String)>,
    revision: u64,
    stop: bool,
    last_status: Value,
    _lock: File,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Params {
    engine: Option<EngineBuild>,
    project: Option<String>,
    path: Option<PathBuf>,
    name: Option<String>,
    template: Option<String>,
    headless: Option<bool>,
    #[serde(default, deserialize_with = "thumbnail")]
    thumbnail: Option<Option<PathBuf>>,
    engine_version: Option<String>,
    dry_run: Option<bool>,
}

fn thumbnail<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<PathBuf>>, D::Error> {
    Option::<PathBuf>::deserialize(deserializer).map(Some)
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}
fn required<T>(value: Option<T>, name: &str) -> Result<T, RpcError> {
    value.ok_or_else(|| invalid(format!("`{name}` is required")))
}

impl Hub {
    /// Lock the user registry. A second service for the same user fails without changing it.
    pub fn open(config: HubConfig) -> Result<Self, RpcError> {
        let updates = UpdateConfig::user(&config.directory)?;
        Self::open_with_updates(config, updates)
    }

    /// Construct a Hub with explicit release dependencies (local fake servers in tests).
    pub fn open_with_updates(
        mut config: HubConfig,
        mut updates: UpdateConfig,
    ) -> Result<Self, RpcError> {
        fs::create_dir_all(&config.directory).map_err(io_error)?;
        config.directory = fs::canonicalize(&config.directory).map_err(io_error)?;
        updates.engines = std::path::absolute(&updates.engines).map_err(io_error)?;
        let lock = File::create(config.directory.join("hub.lock")).map_err(io_error)?;
        lock.try_lock().map_err(|e| {
            io_error(format!(
                "Ara Hub is already running or its lock is unavailable: {e}"
            ))
        })?;
        let mut registry = Registry::load(&config.directory)?;
        let bootstrap = config
            .engine
            .executable
            .parent()
            .is_some_and(|p| p.join("service.json").is_file());
        // A standalone Hub's bootstrap CLI is a service, not an installed engine.
        if !bootstrap {
            if let Some(engine) = registry
                .engines
                .iter_mut()
                .find(|e| e.version == config.engine.version)
            {
                *engine = config.engine.clone();
            } else {
                registry.engines.push(config.engine.clone());
            }
        }
        registry.save(&config.directory)?;
        config.templates =
            Layered::with_checkout_samples(config.templates, &config.engine.executable);
        Ok(Self {
            updates: updates::Updates::new(updates, &config.directory),
            config,
            registry,
            children: BTreeMap::new(),
            routes: BTreeMap::new(),
            subscribers: BTreeSet::new(),
            events: Vec::new(),
            revision: 0,
            stop: false,
            last_status: Value::Null,
            _lock: lock,
        })
    }

    fn changed(&mut self, kind: &str, project: Option<&str>) {
        self.revision += 1;
        let note = jsonrpc::notification(
            "hub.changed",
            json!({"revision":self.revision,"kind":kind,"project":project}),
        )
        .to_string();
        for conn in &self.subscribers {
            self.events.push((*conn, note.clone()));
        }
    }

    fn project(&self, project: Option<String>) -> Result<ProjectEntry, RpcError> {
        Ok(self
            .registry
            .resolve(&required(project, "project")?)?
            .clone())
    }

    /// JSON-RPC methods for the native Hub UI; `hub.call` is the global MCP routing seam.
    pub fn call_for(
        &mut self,
        conn: ConnId,
        method: &str,
        value: Value,
    ) -> Result<Value, RpcError> {
        self.poll_updates();
        if matches!(
            method,
            "hub.engines.available"
                | "hub.updates"
                | "hub.releases.check"
                | "hub.releases.configure"
                | "hub.downloads"
                | "hub.download.cancel"
                | "hub.engine.install"
                | "hub.engine.uninstall"
                | "hub.update.prepare"
        ) {
            return self.update_call(method, value);
        }
        if method == "hub.call" {
            return self.proxy(conn, value);
        }
        let p: Params = parse(value)?;
        if p.path.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err(invalid("path must be absolute"));
        }
        match method {
            "hub.catalog" => {
                if let Some(project) = p.project {
                    let entry = self.registry.resolve(&project)?;
                    if let Some(mut client) = Client::for_project(&entry.path) {
                        return client.call("rpc.discover", json!({}));
                    }
                }
                Ok(json!({"methods":ara_host::core_methods()}))
            }
            "hub.projects" => {
                Ok(json!({"projects":self.registry.projects,"revision":self.revision}))
            }
            "hub.resolve" => Ok(json!(self.project(p.project)?)),
            "hub.templates" => Ok(json!({"templates":self.config.templates.list()?})),
            "hub.engines" => {
                let engines = self
                    .registry
                    .engines
                    .iter()
                    .map(|engine| {
                        let mut value = json!(engine);
                        value["managed"] = json!(
                            engine
                                .executable
                                .parent()
                                .is_some_and(|root| root.join(".ara-managed.json").is_file())
                        );
                        value
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"engines":engines}))
            }
            "hub.engine.register" => {
                let engine = required(p.engine, "engine")?;
                if engine.version.is_empty()
                    || !engine.executable.is_absolute()
                    || !engine.executable.is_file()
                    || engine
                        .editor
                        .as_ref()
                        .is_some_and(|path| !path.is_absolute() || !path.is_file())
                {
                    return Err(invalid(
                        "engine needs a version and existing absolute executable paths",
                    ));
                }
                if let Some(old) = self
                    .registry
                    .engines
                    .iter_mut()
                    .find(|old| old.version == engine.version)
                {
                    *old = engine.clone();
                } else {
                    self.registry.engines.push(engine.clone());
                }
                self.registry.save(&self.config.directory)?;
                self.changed("engine", None);
                Ok(json!(engine))
            }
            "hub.engine.remove" => {
                let engine = required(p.engine, "engine")?;
                if self.registry.projects.iter().any(|project| {
                    project.engine_version == engine.version
                        && self.status_of(project)["state"] == "open"
                }) {
                    return Err(invalid(
                        "close projects using this engine before uninstalling",
                    ));
                }
                self.registry.engines.retain(|old| {
                    old.version != engine.version || old.executable != engine.executable
                });
                self.registry.save(&self.config.directory)?;
                self.changed("engine", None);
                Ok(json!({"removed":engine.version}))
            }
            "hub.register" => {
                let project =
                    self.registry
                        .add(&required(p.path, "path")?, p.name, p.engine_version)?;
                self.registry.save(&self.config.directory)?;
                self.changed("registered", Some(&project.id));
                Ok(json!(project))
            }
            "hub.update" => {
                let old = self.project(p.project)?;
                if let Some(name) = &p.name {
                    self.registry.check_name(name, Some(&old.id))?;
                }
                if let Some(version) = &p.engine_version {
                    self.registry.engine(version)?;
                }
                if p.engine_version.is_some() && self.status_of(&old)["state"] == "open" {
                    return Err(invalid("close the project before changing its engine pin"));
                }
                let entry = self
                    .registry
                    .projects
                    .iter_mut()
                    .find(|e| e.id == old.id)
                    .ok_or_else(|| invalid("project disappeared"))?;
                if let Some(name) = p.name {
                    entry.name = name;
                }
                if let Some(thumbnail) = p.thumbnail {
                    entry.thumbnail = thumbnail;
                }
                if let Some(version) = p.engine_version {
                    entry.engine_version = version;
                }
                let entry = entry.clone();
                let marker_path = entry.path.join("ara.project.json");
                let mut marker: Value =
                    serde_json::from_slice(&fs::read(&marker_path).map_err(io_error)?)
                        .map_err(io_error)?;
                if !marker.is_object() {
                    return Err(invalid("ara.project.json must be an object"));
                }
                marker["name"] = entry.name.clone().into();
                marker["engineVersion"] = entry.engine_version.clone().into();
                write_json(&marker_path, &marker)?;
                self.registry.save(&self.config.directory)?;
                self.changed("updated", Some(&entry.id));
                Ok(json!(entry))
            }
            "hub.remove" => {
                let entry = self.project(p.project)?;
                self.close_project(&entry)?;
                self.registry.projects.retain(|e| e.id != entry.id);
                self.registry.save(&self.config.directory)?;
                self.changed("removed", Some(&entry.id));
                Ok(json!({"removed":entry.id}))
            }
            "hub.create" => {
                let (template, name, path, version) = self.check_create(&p)?;
                if p.dry_run == Some(true) {
                    return Ok(json!({"valid":true,"path":path}));
                }
                fs::create_dir(&path).map_err(io_error)?;
                self.config.templates.instantiate(&template.id, &path)?;
                // Keep the template's preview as the new project's (`thumbnail.png`).
                if let Some(preview) = template.thumbnail
                    && !path.join("thumbnail.png").exists()
                {
                    fs::copy(preview, path.join("thumbnail.png")).map_err(io_error)?;
                }
                let marker_path = path.join("ara.project.json");
                let mut marker: Value =
                    serde_json::from_slice(&fs::read(&marker_path).map_err(io_error)?)
                        .map_err(io_error)?;
                if !marker.is_object() {
                    return Err(invalid("ara.project.json must be an object"));
                }
                marker["name"] = name.clone().into();
                write_json(&marker_path, &marker)?;
                let entry = self.registry.add(&path, Some(name), Some(version))?;
                self.registry.save(&self.config.directory)?;
                self.changed("created", Some(&entry.id));
                Ok(json!(entry))
            }
            "hub.open" => {
                let entry = self.project(p.project)?;
                let mut status = self.open_project(&entry, p.headless.unwrap_or(false))?;
                if let Some(project) = self.registry.projects.iter_mut().find(|p| p.id == entry.id)
                {
                    project.last_opened = Some(now());
                    status["project"] = json!(project);
                }
                self.registry.save(&self.config.directory)?;
                self.changed("opened", Some(&entry.id));
                Ok(status)
            }
            "hub.close" => {
                let entry = self.project(p.project)?;
                self.close_project(&entry)
            }
            "hub.status" => match p.project {
                Some(project) => Ok(self.status_of(self.registry.resolve(&project)?)),
                None => Ok(
                    json!({"projects":self.registry.projects.iter().map(|p|self.status_of(p)).collect::<Vec<_>>(),"revision":self.revision}),
                ),
            },
            "hub.activity" => {
                if let Some(project) = &p.project {
                    self.registry.resolve(project)?;
                }
                let mut activity = Vec::new();
                for entry in &self.registry.projects {
                    if p.project
                        .as_ref()
                        .is_some_and(|id| self.registry.resolve(id).is_ok_and(|e| e.id != entry.id))
                    {
                        continue;
                    }
                    if let Some(mut client) = Client::for_project(&entry.path) {
                        let participants = client.call("collab.activity", json!({}))?;
                        activity.push(json!({"project":entry.id,"participants":participants["participants"],"presence":participants["presence"]}));
                    }
                }
                Ok(json!({"activity":activity}))
            }
            "hub.subscribe" => {
                self.subscribers.insert(conn);
                Ok(json!({"revision":self.revision}))
            }
            "hub.unsubscribe" => {
                self.subscribers.remove(&conn);
                Ok(json!({"ok":true}))
            }
            "hub.shutdown" => {
                self.stop = true;
                Ok(json!({"stopping":true}))
            }
            _ => Err(RpcError {
                code: jsonrpc::METHOD_NOT_FOUND,
                message: format!("unknown Hub method `{method}`"),
                data: None,
            }),
        }
    }
}
fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}
