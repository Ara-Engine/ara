//! Confirm every native commit is represented by disk before metadata-only migration.
use crate::{
    file_state::FileHistory,
    journal::{self, Entry},
};
use ara_diag::{Diagnostic, codes};
use ara_doc::{
    Op,
    file_state::{FileKind, FileState},
};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

fn io(error: impl std::fmt::Display) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::IO, error.to_string())]
}
fn recovery(doc: &str, revision: u64) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::JOURNAL_CORRUPT,
    format!("cannot migrate: committed native edits for `{doc}` through revision {revision} are not confirmed in the project files. No project bytes were changed. Recover and save this journal with the Ara version that authored it, close that authority, then preview migration again; retain .ara/journal and .ara/file-history"))
    .file(".ara/journal")]
}

/// Disk content and identity are inspected without initializing an old project.
struct NativeFile {
    path: String,
    hash: String,
    state: FileState,
}

pub(super) fn validate(root: &Path, history: &FileHistory) -> Result<(), Vec<Diagnostic>> {
    for relative in [".ara", ".ara/journal"] {
        match fs::symlink_metadata(root.join(relative)) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if metadata.file_type().is_symlink() || reparse {
                    return Err(io(format!(
                        "migration refuses linked history path {relative}"
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io(error)),
        }
    }
    let text = fs::read_to_string(root.join(".ara/journal")).map_err(io)?;
    let entries = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<Entry>(line).map_err(|error| {
                vec![
                    Diagnostic::error(
                        codes::JOURNAL_CORRUPT,
                        format!(
                            "repair journal line {} before migrating: {error}",
                            index + 1
                        ),
                    )
                    .file(".ara/journal"),
                ]
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut native = BTreeMap::<String, u64>::new();
    let mut paths = BTreeMap::<String, Vec<String>>::new();
    for entry in &entries {
        match entry {
            Entry::Txn { txn, .. } => {
                for op in &txn.ops {
                    if let Op::File { state, prev, .. } = op {
                        for state in [state, prev].into_iter().flatten() {
                            validate_blob(history, state)?;
                        }
                        continue;
                    }
                    native
                        .entry(op.doc().to_string())
                        .and_modify(|rev| *rev = (*rev).max(txn.result))
                        .or_insert(txn.result);
                    if let Op::Document { doc, path, .. } = op {
                        paths.entry(doc.to_string()).or_default().push(path.clone());
                    }
                }
            }
            Entry::FileFlush {
                state: Some(state), ..
            } => validate_blob(history, state)?,
            _ => {}
        }
    }
    if native.is_empty() {
        return Ok(());
    }
    let mut disk = native_files(root, history, &native)?;
    // Explicit lifecycle paths include deleted documents and custom native extensions.
    for (doc, candidates) in &paths {
        if disk.contains_key(doc) {
            continue;
        }
        for path in candidates {
            ara_doc::file_state::check_path(path).map_err(|d| vec![d])?;
            if history.observe(path).map_err(|d| vec![d])?.is_none() {
                continue;
            }
            if let Some((id, file)) = native_file(root, path, history)? {
                disk.insert(id, file);
            }
        }
    }
    for (doc, latest) in native {
        let mut confirmed = None;
        for entry in &entries {
            match entry {
                Entry::Flush {
                    doc: flushed,
                    rev,
                    hash,
                } if *flushed == doc => {
                    let matches = disk.get(&doc).map_or_else(
                        || {
                            hash == &journal::hash("deleted")
                                && paths.get(&doc).is_some_and(|paths| {
                                    paths.iter().all(|path| !root.join(path).exists())
                                })
                        },
                        |file| hash == &file.hash,
                    );
                    if matches {
                        confirmed = Some(confirmed.map_or(*rev, |before: u64| before.max(*rev)));
                    }
                }
                Entry::FileFlush { path, rev, state } => {
                    let matches = if let Some(file) = disk.get(&doc) {
                        ara_doc::file_state::same_path(path, &file.path)
                            && state.as_ref() == Some(&file.state)
                    } else {
                        state.is_none()
                            && paths.get(&doc).is_some_and(|paths| {
                                paths.iter().any(|candidate| {
                                    ara_doc::file_state::same_path(candidate, path)
                                })
                            })
                            && history.observe(path).map_err(|d| vec![d])?.is_none()
                    };
                    if matches {
                        confirmed = Some(confirmed.map_or(*rev, |before: u64| before.max(*rev)));
                    }
                }
                _ => {}
            }
        }
        if confirmed.is_none_or(|revision| revision < latest) {
            return Err(recovery(&doc, latest));
        }
    }
    Ok(())
}
fn validate_blob(history: &FileHistory, state: &FileState) -> Result<(), Vec<Diagnostic>> {
    state.validate().map_err(|d| vec![d])?;
    if state.kind == FileKind::File {
        history.read(state).map_err(|d| vec![d])?;
    }
    Ok(())
}
fn native_files(
    root: &Path,
    history: &FileHistory,
    touched: &BTreeMap<String, u64>,
) -> Result<BTreeMap<String, NativeFile>, Vec<Diagnostic>> {
    let mut paths = Vec::new();
    super::super::super::migrate::collect(root, root, &mut paths).map_err(|d| vec![d])?;
    let mut files = BTreeMap::new();
    for path in paths {
        let path = path
            .strip_prefix(root)
            .map_err(io)?
            .to_string_lossy()
            .replace('\\', "/");
        if let Some((id, file)) = native_file(root, &path, history)? {
            if !touched.contains_key(&id) {
                continue;
            }
            if files.insert(id.clone(), file).is_some() {
                return Err(vec![Diagnostic::error(codes::JOURNAL_CORRUPT,format!("cannot confirm migration history because native asset ID `{id}` is duplicated on disk")).file(path)]);
            }
        }
    }
    Ok(files)
}
fn native_file(
    root: &Path,
    path: &str,
    history: &FileHistory,
) -> Result<Option<(String, NativeFile)>, Vec<Diagnostic>> {
    let mut file = fs::File::open(root.join(path)).map_err(io)?;
    let mut prefix = [0_u8; 512];
    let count = file.read(&mut prefix).map_err(io)?;
    if !path.ends_with(".ui")
        && prefix[..count]
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|byte| *byte != b'{')
    {
        return Ok(None);
    }
    let text = match fs::read_to_string(root.join(path)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    let id = if path == crate::project::MARKER {
        Some("pj_project".to_owned())
    } else if path == crate::project_layout::PREFERENCES {
        Some(crate::project_layout::PREFERENCES_ID.to_owned())
    } else if path.ends_with(".ui") && !text.trim_start().starts_with('{') {
        crate::ui_document::header(&text, path).map(|(_, id)| id.to_string())
    } else {
        ara_json::parse(&text).ok().and_then(|value| {
            value["id"]
                .as_str()
                .filter(|id| ara_doc::AssetId::parse(id).is_ok())
                .map(str::to_owned)
        })
    };
    let Some(id) = id else {
        return Ok(None);
    };
    let state = FileState::bytes(text.as_bytes());
    if history.observe(path).map_err(|d| vec![d])?.as_ref() != Some(&state) {
        return Err(vec![
            Diagnostic::error(
                codes::STALE_BASE,
                "native file changed while confirming migration journal",
            )
            .file(path),
        ]);
    }
    Ok(Some((
        id,
        NativeFile {
            path: path.to_owned(),
            hash: journal::hash(&text),
            state,
        },
    )))
}
