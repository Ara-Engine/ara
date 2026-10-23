//! `hub.create` validation, shared by creation and its `dryRun`.
use super::*;
use crate::Template;
use ara_diag::{Diagnostic, codes};

impl Hub {
    /// Validate `hub.create` (also its `dryRun`), reporting every problem at once.
    /// Each diagnostic's `location.path` names the field (`name`, `path`,
    /// `template`, `engineVersion`) so the Hub form can show it inline.
    pub(super) fn check_create(
        &self,
        p: &Params,
    ) -> Result<(Template, String, PathBuf, String), RpcError> {
        let field = |path: &str, message: String| {
            Diagnostic::error(codes::INVALID_PARAMS, message).path(path)
        };
        let mut problems = Vec::new();
        let name = p.name.as_deref().map(str::trim).unwrap_or_default();
        if name.is_empty() {
            problems.push(field("name", "Give the project a name".into()));
        } else if self.registry.check_name(name, None).is_err() {
            problems.push(field(
                "name",
                format!("A project named `{name}` is already registered; choose another name"),
            ));
        }
        match &p.path {
            None => problems.push(field("path", "Choose where to create the project".into())),
            Some(path) if path.exists() => problems.push(field(
                "path",
                format!("`{}` already exists; choose a new folder", path.display()),
            )),
            Some(path) if !path.parent().is_some_and(Path::is_dir) => problems.push(field(
                "path",
                format!(
                    "The folder `{}` does not exist",
                    path.parent().unwrap_or(path).display()
                ),
            )),
            Some(_) => {}
        }
        let version = p
            .engine_version
            .clone()
            .unwrap_or_else(|| self.config.engine.version.clone());
        if self.registry.engine(&version).is_err() {
            problems.push(field(
                "engineVersion",
                format!("Engine `{version}` is not installed"),
            ));
        }
        let id = p.template.clone().unwrap_or_default();
        let template = self
            .config
            .templates
            .list()?
            .into_iter()
            .find(|t| t.id == id);
        match &template {
            None => problems.push(field("template", format!("No template `{id}`"))),
            Some(found) => {
                if let Some(error) = found.incomplete() {
                    problems.extend(error.diagnostics());
                }
            }
        }
        match (problems.is_empty(), template, &p.path) {
            (true, Some(template), Some(path)) => {
                Ok((template, name.to_owned(), path.clone(), version))
            }
            _ => Err(RpcError::from_diagnostics(problems)),
        }
    }
}
