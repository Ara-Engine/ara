//! Durable registry. Removal unregisters a project and never deletes its files.
use crate::{invalid, io_error};
use ara_proto::RpcError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A registered project card, shared by Hub UI and MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    /// Stable registry ID; does not change on rename.
    pub id: String,
    /// Unique display name.
    pub name: String,
    /// Canonical absolute project directory.
    pub path: PathBuf,
    /// Last successful open, in Unix milliseconds.
    pub last_opened: Option<u64>,
    /// Optional preview image, supplied by the UI or editor.
    pub thumbnail: Option<PathBuf>,
    /// Pinned installed engine version, also saved in `ara.project.json`.
    pub engine_version: String,
}

/// An installed engine build; versions may coexist.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineBuild {
    /// Version matching a project's `engineVersion` pin.
    pub version: String,
    /// Absolute `ara` CLI executable for this build.
    pub executable: PathBuf,
    /// Optional native editor executable from the same build.
    pub editor: Option<PathBuf>,
}

/// Serialized user registry, independent of running daemon processes.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    /// Registered projects in stable insertion order.
    pub projects: Vec<ProjectEntry>,
    /// Installed builds.
    pub engines: Vec<EngineBuild>,
    #[serde(default)]
    next_id: u64,
}

impl Registry {
    /// Load without silently replacing an unreadable or corrupt registry.
    pub fn load(dir: &Path) -> Result<Self, RpcError> {
        match fs::read(dir.join("registry.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io_error),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(io_error(e)),
        }
    }
    /// Write canonical JSON by replacement; the Hub holds the user-level lock.
    pub fn save(&self, dir: &Path) -> Result<(), RpcError> {
        write_json(
            &dir.join("registry.json"),
            &serde_json::to_value(self).map_err(io_error)?,
        )
    }
    /// Resolve only registered IDs, unique names or canonical paths.
    pub fn resolve(&self, project: &str) -> Result<&ProjectEntry, RpcError> {
        let path = fs::canonicalize(project).ok();
        self.projects
            .iter()
            .find(|p| p.id == project)
            .or_else(|| {
                self.projects
                    .iter()
                    .find(|p| Some(&p.path) == path.as_ref())
            })
            .or_else(|| self.projects.iter().find(|p| p.name == project))
            .ok_or_else(|| invalid(format!("project `{project}` is not registered in Ara Hub")))
    }
    /// Register an existing project and persist its engine pin.
    pub fn add(
        &mut self,
        path: &Path,
        name: Option<String>,
        version: Option<String>,
    ) -> Result<ProjectEntry, RpcError> {
        let path = fs::canonicalize(path).map_err(io_error)?;
        if let Some(p) = self.projects.iter().find(|p| p.path == path) {
            return Ok(p.clone());
        }
        let marker_path = path.join("ara.project.json");
        let mut marker: Value =
            serde_json::from_slice(&fs::read(&marker_path).map_err(io_error)?).map_err(io_error)?;
        if !marker.is_object() {
            return Err(invalid("ara.project.json must be an object"));
        }
        let name = name
            .or_else(|| marker["name"].as_str().map(str::to_owned))
            .ok_or_else(|| invalid("project needs a name"))?;
        self.check_name(&name, None)?;
        let engine_version = version
            .or_else(|| marker["engineVersion"].as_str().map(str::to_owned))
            .or_else(|| self.engines.first().map(|e| e.version.clone()))
            .ok_or_else(|| invalid("no installed engine"))?;
        self.engine(&engine_version)?;
        marker["name"] = name.clone().into();
        marker["engineVersion"] = engine_version.clone().into();
        write_json(&marker_path, &marker)?;
        let id = loop {
            self.next_id += 1;
            let candidate = format!("project-{}", self.next_id);
            if candidate != name
                && !self
                    .projects
                    .iter()
                    .any(|p| p.id == candidate || p.name == candidate)
            {
                break candidate;
            }
        };
        let entry = ProjectEntry {
            id,
            name,
            path,
            last_opened: None,
            thumbnail: None,
            engine_version,
        };
        self.projects.push(entry.clone());
        Ok(entry)
    }
    pub(crate) fn check_name(&self, name: &str, except: Option<&str>) -> Result<(), RpcError> {
        if name.trim().is_empty()
            || self
                .projects
                .iter()
                .any(|p| (p.name == name || p.id == name) && Some(p.id.as_str()) != except)
        {
            return Err(invalid("project name must be nonempty and unique"));
        }
        Ok(())
    }
    pub(crate) fn engine(&self, version: &str) -> Result<&EngineBuild, RpcError> {
        self.engines
            .iter()
            .find(|e| e.version == version)
            .ok_or_else(|| invalid(format!("engine `{version}` is not installed")))
    }
}

pub(crate) fn write_json(path: &Path, value: &Value) -> Result<(), RpcError> {
    let text = ara_json::to_canonical_string(value);
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text).map_err(io_error)?;
    fs::rename(temporary, path).map_err(io_error)
}
