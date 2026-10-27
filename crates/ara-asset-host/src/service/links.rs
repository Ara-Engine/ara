//! Refuse links and Windows reparse points before walking authored content.
use super::inventory::{asset, one};
use ara_asset::Cancellation;
use ara_diag::{Diagnostic, codes};
use std::path::Path;
fn linked(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(super) fn reject_linked_path(root: &Path, relative: &str) -> Result<(), Vec<Diagnostic>> {
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| one(Diagnostic::error(codes::IO, error.to_string()).file(relative)))?;
        if linked(&metadata) {
            return Err(one(Diagnostic::error(
                codes::INVALID_PARAMS,
                "asset inventory refuses symbolic links and reparse points",
            )
            .file(relative)));
        }
    }
    Ok(())
}

pub(super) fn reject_linked_tree(
    root: &Path,
    relative: &str,
    cancel: &Cancellation,
) -> Result<(), Vec<Diagnostic>> {
    let mut pending = vec![relative.to_owned()];
    while let Some(relative) = pending.pop() {
        cancel.check().map_err(asset)?;
        if !relative.is_empty() {
            reject_linked_path(root, &relative)?;
        }
        let directory = root.join(&relative);
        for entry in std::fs::read_dir(directory)
            .map_err(|error| one(Diagnostic::error(codes::IO, error.to_string())))?
        {
            let entry =
                entry.map_err(|error| one(Diagnostic::error(codes::IO, error.to_string())))?;
            let name = entry.file_name().into_string().map_err(|_| {
                one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "authored file names must be UTF-8",
                ))
            })?;
            if relative.is_empty()
                && [
                    ".ara",
                    ".git",
                    "target",
                    "node_modules",
                    "spikes",
                    "builds",
                    "packages",
                ]
                .iter()
                .any(|excluded| ara_doc::file_state::same_path(&name, excluded))
            {
                continue;
            }
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
                one(Diagnostic::error(codes::IO, error.to_string()).file(&path))
            })?;
            if linked(&metadata) {
                return Err(one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "asset inventory refuses symbolic links and reparse points",
                )
                .file(path)));
            }
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(())
}
