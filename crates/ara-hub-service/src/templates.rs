//! Replaceable template source. The alpha includes a blank template and a directory source.
use crate::{invalid, io_error, registry::write_json};
use ara_proto::RpcError;
use serde::Serialize;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A template tile shown by the Hub's create-project view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    /// Stable source-local ID used by `hub.create`.
    pub id: String,
    /// Human-readable title.
    pub name: String,
    /// Optional preview image.
    pub thumbnail: Option<PathBuf>,
}

/// A source can later provide packaged samples or downloaded templates.
pub trait TemplateSource: Send {
    /// Discover available templates without creating a project.
    fn list(&self) -> Result<Vec<Template>, RpcError>;
    /// Populate a newly created empty directory; never overwrite an existing project.
    fn instantiate(&self, template: &str, destination: &Path) -> Result<(), RpcError>;
}

/// Child directories with an `ara.project.json`, plus the built-in `blank` template.
pub struct DirectoryTemplates(pub PathBuf);
impl TemplateSource for DirectoryTemplates {
    fn list(&self) -> Result<Vec<Template>, RpcError> {
        let mut list = vec![Template {
            id: "blank".into(),
            name: "Blank project".into(),
            thumbnail: None,
        }];
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(list),
            Err(e) => return Err(io_error(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            if entry.file_type().map_err(io_error)?.is_dir()
                && entry.path().join("ara.project.json").is_file()
            {
                let id = entry.file_name().to_string_lossy().into_owned();
                if id != "blank" {
                    let thumb = entry.path().join("thumbnail.png");
                    list.push(Template {
                        name: id.clone(),
                        id,
                        thumbnail: thumb.is_file().then_some(thumb),
                    });
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

fn copy_tree(source: &Path, destination: &Path) -> Result<(), RpcError> {
    for entry in fs::read_dir(source).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if [".ara", ".git", "target"]
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
