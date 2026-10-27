//! Complete immutable source/destination facts, prepared only by a worker.
use super::bindings::Bindings;
use super::links::{reject_linked_path, reject_linked_tree};
use ara_asset::{Cancellation, Error, ImportInput};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileKind, FileState, PreparedInventory};
use ara_host::{
    file_state::FileHistory,
    project::{AuthoredFile, AuthoredRole, AuthoredScope},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) struct Inventory {
    pub prepared: PreparedInventory,
    pub authored: Vec<AuthoredFile>,
    root: PathBuf,
    readonly: bool,
}

impl Inventory {
    pub fn read(
        root: &Path,
        bindings: &Bindings,
        readonly: bool,
        cancel: &Cancellation,
    ) -> Result<Self, Vec<Diagnostic>> {
        let history = FileHistory::new(root);
        reject_linked_tree(root, bindings.workspace.reference_root(), cancel)?;
        if !bindings.workspace.reference_root().is_empty() && root.join("settings").exists() {
            reject_linked_tree(root, "settings", cancel)?;
        }
        let authored = ara_host::project::authored_files(root, &bindings.schema).map_err(one)?;
        let mut prepared = PreparedInventory::default();
        prepared
            .directories
            .insert(String::new(), history.directory_entries("").map_err(one)?);
        let prefix = bindings.workspace.reference_root();
        if !prefix.is_empty() {
            prepared
                .entries
                .insert(prefix.to_owned(), history.observe(prefix).map_err(one)?);
            prepared.directories.insert(
                prefix.to_owned(),
                history.directory_entries(prefix).map_err(one)?,
            );
        }
        for file in &authored {
            cancel.check().map_err(asset)?;
            reject_linked_path(root, &file.storage_path)?;
            if file.role == AuthoredRole::SymbolicLink {
                return Err(vec![Diagnostic::error(codes::INVALID_PARAMS, "asset operations refuse links; replace the link with a project-owned file before preparing assets").file(&file.storage_path)]);
            }
            let state = if readonly {
                history.observe(&file.storage_path)
            } else {
                history.snapshot(&file.storage_path)
            }
            .map_err(one)?
            .ok_or_else(|| {
                vec![
                    Diagnostic::error(
                        codes::STALE_BASE,
                        "authored entry disappeared while preparing assets",
                    )
                    .file(&file.storage_path),
                ]
            })?;
            if state.kind == FileKind::Directory {
                prepared.directories.insert(
                    file.storage_path.clone(),
                    history.directory_entries(&file.storage_path).map_err(one)?,
                );
            }
            prepared
                .entries
                .insert(file.storage_path.clone(), Some(state));
        }
        // Files absent from the semantic enumeration still matter for policy
        // fallback and for detecting a legacy policy appearing during the job.
        for path in [
            "ara.project.json",
            "settings/ara.imports",
            "ara.imports",
            ara_host::project_policy::PREFERENCES,
        ] {
            if !prepared.entries.contains_key(path) {
                prepared
                    .entries
                    .insert(path.to_owned(), history.observe(path).map_err(one)?);
            }
        }
        capture_ancestors(&history, &mut prepared, cancel)?;
        history.revalidate(&prepared).map_err(one)?;
        Ok(Self {
            prepared,
            authored,
            root: root.to_owned(),
            readonly,
        })
    }

    pub fn content(&self, storage: &str) -> Result<Vec<u8>, Vec<Diagnostic>> {
        let state = self
            .prepared
            .entries
            .get(storage)
            .and_then(Option::as_ref)
            .filter(|state| state.kind == FileKind::File)
            .ok_or_else(|| {
                vec![
                    Diagnostic::error(
                        codes::STALE_BASE,
                        "asset bytes are missing from the complete inventory",
                    )
                    .file(storage),
                ]
            })?;
        let bytes = if self.readonly {
            std::fs::read(self.root.join(storage)).map_err(|error| {
                one(Diagnostic::error(codes::IO, error.to_string()).file(storage))
            })?
        } else {
            FileHistory::new(&self.root).read(state).map_err(one)?
        };
        if FileState::bytes(&bytes) != *state {
            return Err(one(Diagnostic::error(
                codes::STALE_BASE,
                "file changed after the asset inventory was frozen",
            )
            .file(storage)));
        }
        Ok(bytes)
    }

    pub fn dependencies(
        &self,
        root: &Path,
        bindings: &Bindings,
        worker: &ara_asset::ProcessWorker,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(f32, &str),
    ) -> Result<crate::DependencyInventory, Vec<Diagnostic>> {
        let reference_root = root
            .join(bindings.workspace.reference_root())
            .canonicalize()
            .map_err(|error| one(Diagnostic::error(codes::IO, error.to_string())))?;
        let mut inventory = crate::DependencyInventory::default();
        for (index, file) in self.authored.iter().enumerate() {
            cancel.check().map_err(asset)?;
            if file.scope != AuthoredScope::Authored || file.role != AuthoredRole::Source {
                continue;
            }
            let Some(binding) = bindings.existing(&file.path).map_err(asset)? else {
                continue;
            };
            progress(
                0.1 + 0.35 * index as f32 / self.authored.len().max(1) as f32,
                &format!("Discovering dependencies for {}", file.path),
            );
            let source = self.content(&file.storage_path)?;
            let mut dependencies = BTreeMap::new();
            let parent = reference_root.join(&file.path);
            let parent = parent.parent().ok_or_else(|| {
                one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "source has no parent",
                ))
            })?;
            for uri in worker
                .discover_with_settings(
                    &binding.importer,
                    &binding.version,
                    source.clone(),
                    binding.settings.clone(),
                    cancel,
                )
                .map_err(asset)?
            {
                cancel.check().map_err(asset)?;
                if uri.is_empty() || uri.contains([':', '\\']) || Path::new(&uri).is_absolute() {
                    return Err(one(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "importer dependency must be a local relative URI",
                    )
                    .file(&file.path)));
                }
                let input = parent.join(&uri).canonicalize().map_err(|error| {
                    one(Diagnostic::error(codes::IO, error.to_string()).file(&file.path))
                })?;
                let relative = input
                    .strip_prefix(&reference_root)
                    .map_err(|_| {
                        one(Diagnostic::error(
                            codes::INVALID_PARAMS,
                            "importer dependency escapes the authored root",
                        )
                        .file(&file.path))
                    })?
                    .to_str()
                    .ok_or_else(|| {
                        one(Diagnostic::error(
                            codes::INVALID_PARAMS,
                            "importer dependency path must be UTF-8",
                        ))
                    })?
                    .replace('\\', "/");
                let storage = bindings.workspace.storage_path(&relative);
                dependencies.insert(uri, self.content(&storage)?);
            }
            let discovered = crate::DependencyInventory::from_snapshots(
                &reference_root,
                [ImportInput {
                    preset: binding.preset.unwrap_or(bindings.preset),
                    thread_budget: binding.thread_budget,
                    source_path: file.path.clone(),
                    source,
                    importer: binding.importer,
                    version: binding.version,
                    settings: binding.settings,
                    target: binding.target,
                    dependencies,
                    asset_dependencies: binding.asset_dependencies,
                }],
            )
            .map_err(asset)?;
            // Retain proofs, not source/dependency byte buffers for the whole
            // project. One disposable source snapshot is live at a time.
            for (path, source) in discovered.sources {
                if inventory.sources.contains_key(&path) {
                    return Err(one(Diagnostic::error(
                        codes::INVALID_PARAMS,
                        "duplicate canonical source in complete dependency inventory",
                    )
                    .file(path)));
                }
                inventory.graph.replace(
                    path.clone(),
                    source.inputs.iter().map(|input| input.path.clone()),
                );
                inventory.sources.insert(path, source);
            }
        }
        Ok(inventory)
    }
}

// Semantic enumeration starts inside the authored/settings roots. Freeze those
// roots too, including parents of absent fallback policies, before pure preview
// decides whether a command needs to create a directory.
fn capture_ancestors(
    history: &FileHistory,
    prepared: &mut PreparedInventory,
    cancel: &Cancellation,
) -> Result<(), Vec<Diagnostic>> {
    let mut ancestors = BTreeSet::new();
    for path in prepared.entries.keys() {
        let mut path = path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            ancestors.insert(parent.to_owned());
            path = parent;
        }
    }
    for parent in ancestors {
        cancel.check().map_err(asset)?;
        let state = history.observe(&parent).map_err(one)?;
        if prepared
            .entries
            .get(&parent)
            .is_some_and(|previous| previous != &state)
        {
            return Err(one(Diagnostic::error(
                codes::STALE_BASE,
                "parent changed while preparing the asset inventory",
            )
            .file(&parent)));
        }
        if let Some(state) = &state {
            if state.kind != FileKind::Directory {
                return Err(one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "asset inventory parent must be a directory",
                )
                .file(&parent)));
            }
            if !prepared.directories.contains_key(&parent) {
                prepared.directories.insert(
                    parent.clone(),
                    history.directory_entries(&parent).map_err(one)?,
                );
            }
        }
        prepared.entries.entry(parent).or_insert(state);
    }
    Ok(())
}

pub(super) fn one(diagnostic: Diagnostic) -> Vec<Diagnostic> {
    vec![diagnostic]
}
pub(super) fn asset(error: Error) -> Vec<Diagnostic> {
    vec![error.diagnostic("asset preparation")]
}

#[cfg(test)]
mod tests;
