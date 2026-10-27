//! Resolves UI policy and resource paths at the packaging boundary.
use std::path::{Component, Path, PathBuf};

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Policy {
    pub main: Option<String>,
    pub stylesheet: Option<String>,
    pub bindings: Option<String>,
    pub resources: Option<String>,
}

pub(super) enum Error {
    Missing(String),
    Unsafe(String),
}
impl Error {
    pub fn message(self) -> String {
        match self {
            Self::Missing(message) | Self::Unsafe(message) => message,
        }
    }
}

pub(super) struct Paths {
    root: PathBuf,
    index: Option<ara_doc::AssetIndex>,
}
impl Paths {
    pub fn new(project: &Path, authored: &Path) -> Result<Self, String> {
        let root = authored
            .canonicalize()
            .map_err(|error| format!("UI authored root {}: {error}", authored.display()))?;
        let project_root = project
            .canonicalize()
            .map_err(|error| format!("UI project root: {error}"))?;
        if !root.starts_with(project_root) {
            return Err("UI authored root escapes the project through a link".into());
        }
        let index = if project.join(ara_host::project::MARKER).is_file() {
            Some(
                ara_host::Project::open(project)
                    .map_err(|error| error.to_string())?
                    .index,
            )
        } else {
            None
        };
        Ok(Self { root, index })
    }
    pub fn resolve(&self, value: &str) -> Result<(String, PathBuf), Error> {
        let relative = if value.starts_with('@') {
            let reference = ara_schema::grammar::parse_ref(value).map_err(Error::Unsafe)?;
            reference
                .id
                .and_then(|id| self.index.as_ref()?.path_of(id))
                .or(reference.path)
                .ok_or_else(|| {
                    Error::Unsafe(format!(
                        "UI reference has no resolvable authored path: {value}"
                    ))
                })?
        } else {
            value
        };
        if relative.is_empty()
            || relative.contains([':', '\\'])
            || relative.split('/').any(|part| part.is_empty())
            || !Path::new(relative)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(Error::Unsafe(format!(
                "UI path must stay inside the authored root: {value}"
            )));
        }
        let canonical = self
            .root
            .join(relative)
            .canonicalize()
            .map_err(|error| Error::Missing(format!("UI source {relative}: {error}")))?;
        if !canonical.starts_with(&self.root) || !canonical.is_file() {
            return Err(Error::Unsafe(format!(
                "UI path must be a file inside the authored root: {value}"
            )));
        }
        Ok((relative.to_owned(), canonical))
    }
    pub fn text(&self, value: &str) -> Result<String, String> {
        let (_, path) = self.resolve(value).map_err(Error::message)?;
        std::fs::read_to_string(path).map_err(|error| format!("UI source {value}: {error}"))
    }
}
