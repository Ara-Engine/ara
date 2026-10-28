//! Snapshot planning is read-only unless the explicit worker requests durable blobs.
use super::*;
use crate::file_state::FileHistory;
use ara_cmd::builtin::{MigrationFile, PROJECT_MIGRATION_METADATA, PreparedMigration};
use ara_doc::file_state::{FileKind, FileState, PreparedInventory};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Prepared {
    pub plan: MigrationPlan,
    pub inventory: PreparedInventory,
    pub baselines: Vec<(String, String)>,
}
fn err(error: impl std::fmt::Display) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::IO, error.to_string())]
}
fn relative(root: &Path, path: &Path) -> Result<String, Vec<Diagnostic>> {
    let relative = path
        .strip_prefix(root)
        .map_err(err)?
        .to_string_lossy()
        .replace('\\', "/");
    ara_doc::file_state::check_path(&relative).map_err(|d| vec![d])?;
    Ok(relative)
}
fn walk(
    history: &FileHistory,
    root: &Path,
    directory: &Path,
    inventory: &mut PreparedInventory,
) -> Result<(), Vec<Diagnostic>> {
    let relative = directory
        .strip_prefix(root)
        .map_err(err)?
        .to_string_lossy()
        .replace('\\', "/");
    let children = history.directory_entries(&relative).map_err(|d| vec![d])?;
    inventory
        .directories
        .insert(relative.clone(), children.clone());
    for path in children {
        let state = history.observe(&path).map_err(|d| vec![d])?;
        inventory.entries.insert(path.clone(), state.clone());
        if state.is_some_and(|s| s.kind == FileKind::Directory) {
            let first = path.split('/').next().unwrap_or("");
            if ["target", "builds", "packages", "node_modules", "spikes"].contains(&first)
                || first.starts_with('.')
                || root.join(&path).join(crate::project::MARKER).is_file()
            {
                continue;
            }
            walk(history, root, &root.join(path), inventory)?;
        }
    }
    Ok(())
}
fn parent_paths(path: &str) -> impl Iterator<Item = String> + '_ {
    let mut current = path;
    std::iter::from_fn(move || {
        let (parent, _) = current.rsplit_once('/')?;
        current = parent;
        Some(parent.to_owned())
    })
}

pub(super) fn prepare(migration: &Migration, persist: bool) -> Result<Prepared, Vec<Diagnostic>> {
    let root = &migration.root;
    let history = FileHistory::new(root);
    validate_history(root, &history)?;
    let marker_before: Value =
        ara_json::parse(&fs::read_to_string(root.join(crate::project::MARKER)).map_err(err)?)
            .map_err(|e| err(e.message))?;
    let current = ara_doc::detect_project_layout(&marker_before).is_current();
    let mut inventory = PreparedInventory::default();
    walk(&history, root, root, &mut inventory)?;
    let initial = inventory.entries.clone();
    let mut final_states = initial.clone();
    let mut final_sources = BTreeMap::<String, PathBuf>::new();
    for (path, state) in &initial {
        let target = super::super::migrate::moved_path(&root.join(path), &migration.moves);
        let target = relative(root, &target)?;
        if target != *path {
            final_states.remove(path);
            if final_states.get(&target).is_some_and(Option::is_some) {
                return Err(vec![
                    Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "migration destination collides with existing content",
                    )
                    .file(target),
                ]);
            }
            final_states.insert(target.clone(), state.clone());
        }
        if state.as_ref().is_some_and(|s| s.kind == FileKind::File) {
            final_sources.insert(target, root.join(path));
        }
    }
    let mut bytes = BTreeMap::<String, Vec<u8>>::new();
    for rewrite in &migration.rewrites {
        let source = relative(root, &rewrite.source)?;
        let expected = rewrite.existed.then(|| FileState::bytes(&rewrite.before));
        if initial.get(&source).cloned().flatten() != expected {
            return Err(vec![
                Diagnostic::error(
                    codes::STALE_BASE,
                    "migration source changed during read-only planning",
                )
                .file(source),
            ]);
        }
        let path = relative(root, &rewrite.target)?;
        final_states.insert(path.clone(), Some(FileState::bytes(&rewrite.after)));
        bytes.insert(path, rewrite.after.clone());
    }
    if !current {
        for directory in [
            format!("{}/scenes", ara_doc::AUTHORING_ROOT),
            format!("{}/editor", ara_doc::AUTHORING_ROOT),
            "settings/builds".into(),
            "builds".into(),
            "packages".into(),
        ] {
            final_states
                .entry(directory)
                .or_insert(Some(FileState::directory()));
        }
        // Remove only truly empty legacy ancestors. Nested projects/unmoved support remain.
        let mut cleanup = initial
            .iter()
            .filter(|(path, state)| {
                state
                    .as_ref()
                    .is_some_and(|s| s.kind == FileKind::Directory)
                    && migration
                        .cleanup
                        .iter()
                        .any(|ancestor| root.join(path).starts_with(ancestor))
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        cleanup.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));
        for path in cleanup {
            let prefix = format!("{path}/");
            if !final_states
                .iter()
                .any(|(p, s)| p.starts_with(&prefix) && s.is_some())
            {
                final_states.remove(&path);
            }
        }
    }
    for path in final_states.keys().cloned().collect::<Vec<_>>() {
        for parent in parent_paths(&path) {
            if final_states
                .get(&parent)
                .and_then(Option::as_ref)
                .is_some_and(|s| s.kind != FileKind::Directory)
            {
                return Err(vec![
                    Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "migration needs a directory where a file exists",
                    )
                    .file(parent),
                ]);
            }
            final_states.insert(parent, Some(FileState::directory()));
        }
    }
    let all = initial
        .keys()
        .chain(final_states.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut files = Vec::new();
    for path in all {
        let before = initial.get(&path).cloned().flatten();
        let after = final_states.get(&path).cloned().flatten();
        inventory.entries.entry(path.clone()).or_insert(None);
        if before != after {
            files.push(MigrationFile {
                path,
                before,
                after,
            });
        }
    }
    // Parent directories exist before children; inverse order restores old children before pruning new parents.
    files.sort_by_key(|f| {
        let depth = f.path.matches('/').count();
        match &f.after {
            Some(s) if s.kind == FileKind::Directory => (0, depth, f.path.clone()),
            Some(_) => (1, 0, f.path.clone()),
            None => (2, usize::MAX - depth, f.path.clone()),
        }
    });
    let marker_after = bytes
        .get(crate::project::MARKER)
        .map(|bytes| serde_json::from_slice(bytes))
        .transpose()
        .map_err(err)?
        .unwrap_or_else(|| marker_before.clone());
    let token_bytes = ara_json::to_canonical_string(
        &json!({"version":super::super::VERSION,"entries":inventory.entries,"directories":inventory.directories,"files":files,"marker":marker_after}),
    );
    let token = FileState::bytes(token_bytes.as_bytes()).hash;
    inventory.metadata = json!({PROJECT_MIGRATION_METADATA:PreparedMigration{plan_token:token.clone(),marker_before:marker_before.clone(),marker_after,files:files.clone()}});
    let operations = migration
        .operations
        .iter()
        .map(|op| match op {
            MigrationOp::Move { from, to } => Ok(MigrationOperation::Move {
                from: relative(root, from)?,
                to: relative(root, to)?,
            }),
            MigrationOp::Rewrite { path } => Ok(MigrationOperation::Rewrite {
                path: relative(root, path)?,
            }),
        })
        .collect::<Result<Vec<_>, Vec<Diagnostic>>>()?;
    let mut baselines = Vec::new();
    for file in &files {
        if persist {
            if file
                .before
                .as_ref()
                .is_some_and(|s| s.kind == FileKind::File)
            {
                let state = history.snapshot(&file.path).map_err(|d| vec![d])?;
                if state != file.before {
                    return Err(vec![
                        Diagnostic::error(
                            codes::STALE_BASE,
                            "source changed while storing migration history",
                        )
                        .file(&file.path),
                    ]);
                }
            }
            if file
                .after
                .as_ref()
                .is_some_and(|s| s.kind == FileKind::File)
            {
                let state = if let Some(bytes) = bytes.get(&file.path) {
                    history.put(bytes).map_err(|d| vec![d])?
                } else {
                    let source = final_sources
                        .get(&file.path)
                        .ok_or_else(|| err("missing migration source"))?;
                    history
                        .snapshot(&relative(root, source)?)
                        .map_err(|d| vec![d])?
                        .ok_or_else(|| err("migration source disappeared"))?
                };
                if Some(&state) != file.after.as_ref() {
                    return Err(vec![
                        Diagnostic::error(
                            codes::STALE_BASE,
                            "migration output changed during preparation",
                        )
                        .file(&file.path),
                    ]);
                }
            }
        }
        if file
            .after
            .as_ref()
            .is_some_and(|s| s.kind == FileKind::File)
            && matches!(
                Path::new(&file.path)
                    .extension()
                    .and_then(|ext| ext.to_str()),
                Some(
                    "json"
                        | "scene"
                        | "prefab"
                        | "animation"
                        | "terrain"
                        | "foliage"
                        | "music"
                        | "mixer"
                        | "material"
                        | "graph"
                        | "world"
                        | "ui"
                        | "input"
                        | "nav"
                        | "vfx"
                        | "look"
                )
            )
        {
            let text = if let Some(bytes) = bytes.get(&file.path) {
                String::from_utf8(bytes.clone()).ok()
            } else {
                final_sources
                    .get(&file.path)
                    .and_then(|path| fs::read_to_string(path).ok())
            };
            if let Some(text) = text {
                let id = if file.path == crate::project::MARKER {
                    Some("pj_project".into())
                } else if file.path == super::super::PREFERENCES {
                    Some(super::super::PREFERENCES_ID.into())
                } else if file.path.ends_with(".ui") {
                    crate::ui_document::header(&text, &file.path).map(|(_, id)| id.to_string())
                } else {
                    serde_json::from_str::<Value>(&text)
                        .ok()
                        .filter(|v| v.get("$schema").is_some() && v.get("records").is_some())
                        .and_then(|v| v["id"].as_str().map(str::to_owned))
                };
                if let Some(id) = id {
                    baselines.push((id, crate::journal::hash(&text)));
                }
            }
        }
    }
    history.revalidate(&inventory).map_err(|d| vec![d])?;
    if !current {
        validate_command(root, &inventory, &token)?;
    }
    Ok(Prepared {
        plan: MigrationPlan {
            needs_migration: !current,
            from_version: marker_before["layoutVersion"].as_u64().unwrap_or(0),
            to_version: super::super::VERSION,
            authored_root: ara_doc::AUTHORING_ROOT.into(),
            plan_token: token,
            operations,
            preview_tree: final_states
                .into_iter()
                .filter(|(_, s)| s.is_some())
                .map(|(p, _)| p)
                .collect(),
        },
        inventory,
        baselines,
    })
}

/// Run the actual command in rollback mode; a preview can never create .ara/.
fn validate_command(
    root: &Path,
    inventory: &PreparedInventory,
    token: &str,
) -> Result<(), Vec<Diagnostic>> {
    let project = crate::Project::migration_bootstrap(root).map_err(|d| vec![d])?;
    let text = fs::read_to_string(root.join(crate::project::MARKER)).map_err(err)?;
    let marker = crate::project::read_document(&text, crate::project::MARKER, &project.db, None)
        .doc
        .ok_or_else(|| err("invalid migration marker"))?;
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert_project_document(crate::project::MARKER, marker);
    let mut store = ara_ops::Store::new(project.db, workspace);
    store
        .prepare_files(inventory.clone())
        .map_err(|d| vec![d])?;
    let mut batch = ara_cmd::Batch::new(
        "pj_project",
        [("project.migrate", json!({"planToken":token}))],
    );
    batch.dry_run = true;
    project
        .registry
        .apply(&mut store, &batch, &ara_cmd::Session::human())?;
    Ok(())
}

#[path = "journal_safety.rs"]
mod journal_safety;

/// Migration cannot hide committed native edits behind a newer file baseline.
fn validate_history(root: &Path, history: &FileHistory) -> Result<(), Vec<Diagnostic>> {
    journal_safety::validate(root, history)
}
