//! The Hub RPCs the view-model issues. The connection worker runs them off the UI
//! thread and hands each result back with the call that produced it.
use super::wire::Engine;
use serde_json::{Value, json};
use std::path::PathBuf;

/// One `hub.*` request.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// Release-service method with its validated wire parameters.
    Release { method: &'static str, params: Value },
    /// `hub.projects`.
    Projects,
    /// `hub.status` for every project.
    Status,
    /// `hub.templates`.
    Templates,
    /// `hub.engines`.
    Engines,
    /// `hub.activity` for every open project.
    Activity,
    /// `hub.register`: add an existing project folder.
    Register {
        /// Absolute project directory.
        path: PathBuf,
    },
    /// `hub.create`; `dry_run` validates without creating (the form's live check).
    Create {
        /// Template ID.
        template: String,
        /// Project name.
        name: String,
        /// Absolute new project directory; absent while the destination is unusable.
        path: Option<PathBuf>,
        /// Engine pin; the service's own version when absent.
        engine: Option<String>,
        /// Validate only.
        dry_run: bool,
        /// Form revision this check belongs to.
        revision: u64,
    },
    /// `hub.open`.
    Open {
        /// Registry ID.
        project: String,
        /// Background daemon instead of the native editor.
        headless: bool,
    },
    /// `hub.close`.
    Close {
        /// Registry ID.
        project: String,
    },
    /// `hub.update` with a new name.
    Rename {
        /// Registry ID.
        project: String,
        /// New unique name.
        name: String,
    },
    /// `hub.update` with a new engine pin.
    Pin {
        /// Registry ID.
        project: String,
        /// Installed version.
        version: String,
    },
    /// `hub.remove`: unregister, keeping files.
    Remove {
        /// Registry ID.
        project: String,
    },
    /// `hub.engine.register`.
    EngineRegister(Engine),
    /// `hub.engine.remove`.
    EngineRemove(Engine),
}

impl Call {
    /// JSON-RPC method name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Release { method, .. } => method,
            Self::Projects => "hub.projects",
            Self::Status => "hub.status",
            Self::Templates => "hub.templates",
            Self::Engines => "hub.engines",
            Self::Activity => "hub.activity",
            Self::Register { .. } => "hub.register",
            Self::Create { .. } => "hub.create",
            Self::Open { .. } => "hub.open",
            Self::Close { .. } => "hub.close",
            Self::Rename { .. } | Self::Pin { .. } => "hub.update",
            Self::Remove { .. } => "hub.remove",
            Self::EngineRegister(_) => "hub.engine.register",
            Self::EngineRemove(_) => "hub.engine.remove",
        }
    }
    /// camelCase params, per the wire contract.
    pub fn params(&self) -> Value {
        match self {
            Self::Release { params, .. } => params.clone(),
            Self::Projects | Self::Status | Self::Templates | Self::Engines | Self::Activity => {
                json!({})
            }
            Self::Register { path } => json!({ "path": path }),
            Self::Create {
                template,
                name,
                path,
                engine,
                dry_run,
                ..
            } => {
                let mut params = json!({"template":template,"name":name});
                if let Some(path) = path {
                    params["path"] = json!(path);
                }
                if let Some(engine) = engine {
                    params["engineVersion"] = json!(engine);
                }
                if *dry_run {
                    params["dryRun"] = json!(true);
                }
                params
            }
            Self::Open { project, headless } => json!({"project":project,"headless":headless}),
            Self::Close { project } | Self::Remove { project } => json!({ "project": project }),
            Self::Rename { project, name } => json!({"project":project,"name":name}),
            Self::Pin { project, version } => json!({"project":project,"engineVersion":version}),
            Self::EngineRegister(engine) | Self::EngineRemove(engine) => {
                json!({ "engine": engine })
            }
        }
    }
}
