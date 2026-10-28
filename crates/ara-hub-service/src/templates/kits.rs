//! Required generated sources come from the kit's checked-in import sidecars.
use super::TemplateSetup;
use crate::io_error;
use ara_proto::RpcError;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Component, Path, PathBuf},
};

pub(super) fn missing(
    id: &str,
    root: &Path,
    name: &str,
) -> Result<Option<TemplateSetup>, RpcError> {
    let authored = ara_host::project_layout::authored_root(root);
    let kit = if authored == root {
        authored.join("assets/kit")
    } else {
        authored.join("kit")
    };
    let mut missing = BTreeSet::new();
    inspect(&kit, &kit, &mut missing).map_err(io_error)?;
    Ok((!missing.is_empty()).then(|| {
        let examples = missing
            .iter()
            .take(3)
            .map(|path| path.strip_prefix(root).unwrap_or(path).display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let kind = if missing.len() == 1 { "file" } else { "files" };
        TemplateSetup {
            message: format!(
                "{name} needs its generated kits: {} required {kind} missing or incomplete ({examples})",
                missing.len()
            ),
            command: format!("cargo run -p ara-samples -- kits {id}"),
        }
    }))
}

fn inspect(kit: &Path, directory: &Path, missing: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            inspect(kit, &entry.path(), missing)?;
            continue;
        }
        let filename = entry.file_name();
        let Some(source) = filename
            .to_str()
            .and_then(|s| s.strip_suffix(".import.json"))
        else {
            continue;
        };
        let source = directory.join(source);
        if !source.is_file() {
            missing.insert(source);
        } else if source.extension().is_some_and(|ext| ext == "gltf") {
            let Ok(model) = serde_json::from_slice::<Value>(&fs::read(&source)?) else {
                missing.insert(source);
                continue;
            };
            for group in ["buffers", "images"] {
                for item in model[group].as_array().into_iter().flatten() {
                    if let Some(path) = item["uri"]
                        .as_str()
                        .and_then(|uri| local_dependency(kit, directory, uri))
                        && !path.is_file()
                    {
                        missing.insert(path);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Only kit-local companions: embedded data and fetched third-party art belong
/// to their own paths/checks, never to the kit generator's required output set.
fn local_dependency(kit: &Path, directory: &Path, uri: &str) -> Option<PathBuf> {
    if uri.contains(':') {
        return None;
    }
    let mut path = directory.to_owned();
    for part in Path::new(uri).components() {
        match part {
            Component::Normal(name) => path.push(name),
            Component::CurDir => {}
            Component::ParentDir if path != kit => {
                path.pop();
            }
            _ => return None,
        }
    }
    Some(path)
}
