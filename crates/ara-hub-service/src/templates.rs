//! Replaceable template source. The alpha includes a blank template and a directory source.
use crate::{invalid, io_error, registry::write_json};
use ara_diag::{Diagnostic, codes};
use ara_proto::RpcError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A template tile shown by the Hub's create-project view.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    /// Stable source-local ID used by `hub.create`.
    pub id: String,
    /// Human-readable title.
    pub name: String,
    /// Optional preview image.
    pub thumbnail: Option<PathBuf>,
    /// Optional one-line summary from the template's `ara.project.json` `description`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Setup still needed before the template can be created, such as fetching
    /// sample art. `hub.create` rejects the template while this is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<TemplateSetup>,
}

/// A missing prerequisite of a template and the command that restores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSetup {
    /// What is missing, for people.
    pub message: String,
    /// The command that restores it, run from an Ara source checkout.
    pub command: String,
}

impl Template {
    /// The error `hub.create` returns for a template that still needs setup.
    pub(crate) fn incomplete(&self) -> Option<RpcError> {
        let setup = self.setup.as_ref()?;
        Some(
            Diagnostic::error(
                codes::TEMPLATE_INCOMPLETE,
                format!(
                    "{}. Run `{}` in an Ara source checkout, or pick another template",
                    setup.message, setup.command
                ),
            )
            .path("template")
            .into(),
        )
    }
}

/// A source can later provide packaged samples or downloaded templates.
pub trait TemplateSource: Send {
    /// Discover available templates without creating a project.
    fn list(&self) -> Result<Vec<Template>, RpcError>;
    /// Populate a newly created empty directory; never overwrite an existing project.
    fn instantiate(&self, template: &str, destination: &Path) -> Result<(), RpcError>;
}

/// Child directories with an `ara.project.json`, plus the built-in `blank` template.
///
/// Names and descriptions come from each template's `ara.project.json`; the preview
/// is `thumbnail.png`, else the Play capture of its main scene
/// (`evidence/<main-scene>-player.png`). An `assets.lock.json` beside the
/// templates (the `samples/` lock) marks templates whose pinned third-party files
/// are not present yet; those carry a [`TemplateSetup`].
pub struct DirectoryTemplates(pub PathBuf);
impl TemplateSource for DirectoryTemplates {
    fn list(&self) -> Result<Vec<Template>, RpcError> {
        let mut list = vec![Template {
            id: "blank".into(),
            name: "Blank project".into(),
            description: Some("An empty scene, ready for your first entity.".into()),
            ..Template::default()
        }];
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(list),
            Err(e) => return Err(io_error(e)),
        };
        let lock = Lock::read(&self.0.join("assets.lock.json"));
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            let marker = entry.path().join("ara.project.json");
            if entry.file_type().map_err(io_error)?.is_dir() && marker.is_file() {
                let id = entry.file_name().to_string_lossy().into_owned();
                if id != "blank" {
                    list.push(describe(&id, &entry.path(), &marker, lock.as_ref()));
                }
            }
        }
        list[1..].sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }
    fn instantiate(&self, template: &str, destination: &Path) -> Result<(), RpcError> {
        if template == "blank" {
            fs::create_dir(destination.join("levels")).map_err(io_error)?;
            fs::write(destination.join(".gitignore"), ".ara/\n").map_err(io_error)?;
            write_json(
                &destination.join("ara.project.json"),
                &json!({"name":"New project","main":"levels/main.scene"}),
            )?;
            write_json(
                &destination.join("levels/main.scene"),
                &json!({"$schema":"ara://schema/scene/1","id":"sc_main001","records":{}}),
            )?;
            return Ok(());
        }
        if !self.list()?.iter().any(|t| t.id == template) {
            return Err(invalid(format!("no template `{template}`")));
        }
        copy_tree(&self.0.join(template), destination)
    }
}

fn describe(id: &str, root: &Path, marker: &Path, lock: Option<&Lock>) -> Template {
    let marker: Value = fs::read(marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let text = |key: &str| {
        marker[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let main = marker["main"]
        .as_str()
        .and_then(|main| Path::new(main).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned());
    let thumbnail = std::iter::once(root.join("thumbnail.png"))
        .chain(main.map(|stem| root.join(format!("evidence/{stem}-player.png"))))
        .find(|path| path.is_file());
    let name = text("name").unwrap_or_else(|| id.to_owned());
    Template {
        id: id.to_owned(),
        setup: lock.and_then(|lock| lock.missing(id, root, &name)),
        name,
        thumbnail,
        description: text("description"),
    }
}

/// The subset of `samples/assets.lock.json` needed to detect unfetched art.
#[derive(Deserialize)]
struct Lock {
    assets: Vec<LockedAsset>,
}
#[derive(Deserialize)]
struct LockedAsset {
    id: String,
    projects: Vec<String>,
    files: Vec<LockedFile>,
}
#[derive(Deserialize)]
struct LockedFile {
    path: String,
}
impl Lock {
    fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }
    /// Setup for a template whose locked files are not all under `assets/third_party/`.
    fn missing(&self, id: &str, root: &Path, name: &str) -> Option<TemplateSetup> {
        let files: Vec<_> = self
            .assets
            .iter()
            .filter(|asset| asset.projects.iter().any(|p| p == id))
            .flat_map(|asset| {
                asset.files.iter().map(move |file| {
                    root.join("assets/third_party")
                        .join(&asset.id)
                        .join(&file.path)
                })
            })
            .collect();
        let missing = files.iter().filter(|path| !path.is_file()).count();
        (missing > 0).then(|| TemplateSetup {
            message: format!(
                "{name} needs its third-party art: {missing} of {} pinned files are not downloaded",
                files.len()
            ),
            command: format!("ara-samples fetch {id}"),
        })
    }
}

/// The installed or configured templates, then the samples of the source checkout
/// the engine runs from (`<checkout>/target/<profile>/ara`). IDs listed first win.
pub(crate) struct Layered {
    pub first: Box<dyn TemplateSource>,
    pub samples: DirectoryTemplates,
}
impl Layered {
    /// Add the checkout's `samples/` when `executable` is a build inside a source checkout.
    pub(crate) fn with_checkout_samples(
        templates: Box<dyn TemplateSource>,
        executable: &Path,
    ) -> Box<dyn TemplateSource> {
        let checkout = executable
            .parent()
            .and_then(Path::parent)
            .filter(|target| target.file_name().is_some_and(|n| n == "target"))
            .and_then(Path::parent)
            .filter(|root| {
                root.join("Cargo.toml").is_file() && root.join("samples/assets.lock.json").is_file()
            });
        match checkout {
            Some(root) => Box::new(Self {
                first: templates,
                samples: DirectoryTemplates(root.join("samples")),
            }),
            None => templates,
        }
    }
}
impl TemplateSource for Layered {
    fn list(&self) -> Result<Vec<Template>, RpcError> {
        let mut list = self.first.list()?;
        for template in self.samples.list()? {
            if !list.iter().any(|t| t.id == template.id) {
                list.push(template);
            }
        }
        Ok(list)
    }
    fn instantiate(&self, template: &str, destination: &Path) -> Result<(), RpcError> {
        if self.first.list()?.iter().any(|t| t.id == template) {
            self.first.instantiate(template, destination)
        } else {
            self.samples.instantiate(template, destination)
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), RpcError> {
    for entry in fs::read_dir(source).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        // Machine state and capture output never become part of a new project.
        if [".ara", ".git", "target", "evidence"]
            .iter()
            .any(|n| entry.file_name() == *n)
        {
            continue;
        }
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_symlink() {
            return Err(invalid("templates cannot contain symbolic links"));
        }
        let dest = destination.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&dest).map_err(io_error)?;
            copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), dest).map_err(io_error)?;
        }
    }
    Ok(())
}
