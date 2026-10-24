//! The Create form. Validation is the service's own: every edit runs a
//! `hub.create` dry run, and its field-tagged diagnostics (`location.path` =
//! `name`, `path`, `template` or `engineVersion`) render under their fields.
use super::{Call, Engine, Project, Template, text};
use ara_diag::{Diagnostic, codes};
use ara_proto::RpcError;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Create-view inputs and their latest validation.
pub struct CreateForm {
    /// Selected template ID.
    pub template: Option<String>,
    /// Project name.
    pub name: String,
    /// True once the person typed a name (stops the suggested name).
    pub named: bool,
    /// Parent folder for the new project folder.
    pub destination: String,
    /// Engine pin.
    pub engine: Option<String>,
    /// Diagnostics of the latest check, local and from the service.
    pub problems: Vec<Diagnostic>,
    /// The real `hub.create` is in flight.
    pub submitting: bool,
    revision: u64,
    checking: Option<u64>,
    checked: Option<u64>,
    /// An older Hub service rejected `dryRun`: check locally, and fully on create.
    no_dry_run: bool,
}

impl CreateForm {
    /// An empty form whose destination starts at `home`.
    pub fn new(home: PathBuf) -> Self {
        Self {
            template: None,
            name: String::new(),
            named: false,
            destination: text::path(&home),
            engine: None,
            problems: Vec::new(),
            submitting: false,
            revision: 0,
            checking: None,
            checked: None,
            no_dry_run: false,
        }
    }

    /// The folder the project will be created in.
    pub fn path(&self) -> Option<PathBuf> {
        let destination = Path::new(self.destination.trim());
        let folder = text::folder(&self.name);
        (destination.is_absolute() && !folder.is_empty()).then(|| destination.join(folder))
    }

    /// Fill defaults from fresh lists, then mark the inputs changed.
    pub(super) fn validate(
        &mut self,
        projects: &[Project],
        engines: &[Engine],
        templates: &[Template],
    ) {
        if self
            .template
            .as_ref()
            .is_none_or(|id| !templates.iter().any(|t| &t.id == id))
        {
            self.template = templates
                .iter()
                .find(|t| t.setup.is_none())
                .or(templates.first())
                .map(|t| t.id.clone());
        }
        if !self.named {
            let destination = PathBuf::from(self.destination.trim());
            self.name = text::unique("My project", |name| {
                projects.iter().any(|p| p.name == name)
                    || destination.join(text::folder(name)).exists()
            });
        }
        if self
            .engine
            .as_ref()
            .is_none_or(|v| !engines.iter().any(|e| &e.version == v))
        {
            self.engine = engines
                .iter()
                .find(|e| e.version == ara_hub_service::ALPHA_VERSION)
                .or(engines.first())
                .map(|e| e.version.clone());
        }
        self.revision += 1;
    }

    /// Problems found without the service (an unusable destination).
    fn local(&self) -> Vec<Diagnostic> {
        let destination = self.destination.trim();
        let message = if destination.is_empty() {
            "Choose where to create the project"
        } else if !Path::new(destination).is_absolute() {
            "Use a full folder path, such as C:\\Games"
        } else {
            return Vec::new();
        };
        vec![Diagnostic::error(codes::INVALID_PARAMS, message).path("path")]
    }

    fn call(&self, dry_run: bool) -> Option<Call> {
        Some(Call::Create {
            template: self.template.clone()?,
            name: self.name.trim().to_owned(),
            path: self.path(),
            engine: self.engine.clone(),
            dry_run,
            revision: self.revision,
        })
    }

    /// A dry run for the current inputs, unless one is already in flight.
    pub(super) fn check(&mut self) -> Option<Call> {
        if self.checking.is_some() || self.submitting {
            return None;
        }
        if self.no_dry_run {
            self.problems = self.local();
            self.checked = Some(self.revision);
            return None;
        }
        let call = self.call(true)?;
        self.checking = Some(self.revision);
        Some(call)
    }

    /// Record a dry-run result; returns the next check when the inputs moved on.
    pub(super) fn checked(&mut self, call: &Call, result: Result<Value, RpcError>) -> Option<Call> {
        self.checking = None;
        let Call::Create { revision, .. } = call else {
            return None;
        };
        if *revision != self.revision {
            return self.check();
        }
        let local = self.local();
        let mut problems = local.clone();
        if let Err(error) = &result
            && error.message.contains("dryRun")
        {
            self.no_dry_run = true;
            self.problems = local;
            self.checked = Some(*revision);
            return None;
        }
        if let Err(error) = result {
            problems.extend(error.diagnostics().into_iter().filter(|d| {
                // A local path problem replaces the service's "choose a folder".
                local.is_empty() || d.location.path.as_deref() != Some("path")
            }));
        }
        self.problems = problems;
        self.checked = Some(*revision);
        None
    }

    /// True while a check or the creation itself is pending.
    pub fn waiting(&self) -> bool {
        self.checking.is_some() || self.submitting
    }
    /// Whether Create is enabled: no known problems and nothing pending.
    pub fn can_submit(&self) -> bool {
        !self.submitting
            && self.local().is_empty()
            && (self.checked != Some(self.revision) || self.problems.is_empty())
    }
    /// The real `hub.create`, when the form allows it.
    pub(super) fn submit(&mut self) -> Option<Call> {
        if !self.can_submit() {
            return None;
        }
        let call = self.call(false)?;
        self.submitting = true;
        Some(call)
    }
    /// Record the creation result; a success resets the name for the next project.
    pub(super) fn created(&mut self, result: Result<Value, RpcError>) -> Result<Project, RpcError> {
        self.submitting = false;
        match result {
            Ok(value) => {
                self.named = false;
                self.problems.clear();
                serde_json::from_value(value)
                    .map_err(|e| Diagnostic::error(codes::INVALID_PARAMS, e.to_string()).into())
            }
            Err(error) => {
                self.problems = error.diagnostics();
                self.checked = Some(self.revision);
                Err(error)
            }
        }
    }

    /// Problems for one field (`name`, `path`, `template`, `engineVersion`).
    pub fn problems_for<'a>(&'a self, field: &'a str) -> impl Iterator<Item = &'a Diagnostic> {
        self.problems
            .iter()
            .filter(move |d| d.location.path.as_deref() == Some(field))
    }
    /// Problems not tied to a field.
    pub fn general(&self) -> impl Iterator<Item = &Diagnostic> {
        self.problems.iter().filter(|d| {
            !matches!(
                d.location.path.as_deref(),
                Some("name" | "path" | "template" | "engineVersion")
            )
        })
    }
}
