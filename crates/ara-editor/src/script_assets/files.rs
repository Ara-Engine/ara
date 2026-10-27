//! File operations emitted by typed asset commands. Source code stays in the
//! project's worktree (D21); authoring documents continue through kernel ops.
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

/// Reviewable source-file output of one typed authoring command.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub(super) enum FileOp {
    Create {
        path: String,
        text: String,
    },
    Folder {
        path: String,
    },
    Rename {
        from: String,
        path: String,
        text: String,
    },
}

pub(super) fn checked(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || Path::new(relative)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(
            "Use a project-relative path with forward slashes and no parent traversal".into(),
        );
    }
    let path = root.join(relative);
    let canonical_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    for ancestor in path.ancestors().take_while(|p| *p != root) {
        if ancestor.exists()
            && !std::fs::canonicalize(ancestor)
                .map_err(|e| e.to_string())?
                .starts_with(&canonical_root)
        {
            return Err("Asset path escapes the project through a link".into());
        }
    }
    Ok(path)
}

pub(super) fn apply(root: &Path, ops: &[FileOp]) -> Result<(), String> {
    use std::io::Write;
    for op in ops {
        let relative = match op {
            FileOp::Create { path, .. } | FileOp::Folder { path } | FileOp::Rename { path, .. } => {
                path
            }
        };
        let path = checked(root, relative)?;
        if path.exists() {
            return Err(format!("{relative} already exists"));
        }
        if let FileOp::Rename { from, .. } = op {
            checked(root, from)?;
        }
    }
    let mut created = Vec::new();
    let result = (|| {
        for op in ops {
            match op {
                FileOp::Folder { path } => {
                    let path = checked(root, path)?;
                    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
                    created.push(path);
                }
                FileOp::Create { path, text } | FileOp::Rename { path, text, .. } => {
                    let path = checked(root, path)?;
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|e| e.to_string())?;
                    created.push(path);
                    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
                }
            }
        }
        for op in ops {
            if let FileOp::Rename { from, .. } = op {
                std::fs::remove_file(checked(root, from)?).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        for path in created.into_iter().rev() {
            if path.is_dir() {
                let _ = std::fs::remove_dir(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    result
}
