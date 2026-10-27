//! Immutable source-to-input graphs prepared away from the editor's frame thread.
#![deny(missing_docs)]

pub mod import;
pub mod service;
pub use service::AssetService;

use ara_asset::{
    Cancellation, DependencyGraph, Digest, Error, ImportInput, ProcessWorker, Result, SourceAsset,
};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

/// One original model URI and its canonical local file, relative to the authored root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInput {
    /// URI spelling used by the importer, retained for companion relocation.
    pub uri: String,
    /// Canonical local path with `/` separators, inside the authored root.
    pub path: String,
    /// Digest of the immutable bytes supplied to the importer.
    pub digest: Digest,
}

/// A source and all local URI inputs discovered from its immutable bytes/settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDependencies {
    /// Original source spelling, re-resolved during verification to detect retargeted links.
    pub original_path: String,
    /// Canonical consumer source path, relative to the authored root.
    pub path: String,
    /// Digest of the consumer bytes used during discovery.
    pub digest: Digest,
    /// Sorted by URI, including separate aliases for the same input file.
    pub inputs: Vec<LocalInput>,
}

/// Read-only dependency graph plus the exact source/input hashes it describes.
#[derive(Debug, Clone, Default)]
pub struct DependencyInventory {
    sources: BTreeMap<String, SourceDependencies>,
    graph: DependencyGraph,
}

impl DependencyInventory {
    /// Discovers every provided source through existing disposable importer workers.
    /// Call on a background thread with a complete authored-source inventory.
    pub fn prepare(
        project_root: &Path,
        reference_root: &Path,
        sources: &[SourceAsset],
        worker: &ProcessWorker,
        cancel: &Cancellation,
    ) -> Result<Self> {
        let project_root = project_root.canonicalize()?;
        let reference_root = reference_root.canonicalize()?;
        if !reference_root.starts_with(&project_root) {
            return Err(Error::Invalid(
                "reference root is outside the project".into(),
            ));
        }
        let mut inputs = Vec::with_capacity(sources.len());
        for source in sources {
            cancel.check()?;
            inputs.push(ara_asset::snapshot(&project_root, source, worker, cancel)?);
        }
        Self::from_snapshots(&reference_root, inputs)
    }

    /// Reuses already prepared importer inputs without parsing or cooking a model.
    pub fn from_snapshots(
        root: &Path,
        inputs: impl IntoIterator<Item = ImportInput>,
    ) -> Result<Self> {
        let root = root.canonicalize()?;
        let mut inventory = Self::default();
        for input in inputs {
            let path = local_path(&root, &root, &input.source_path)?;
            let parent = root.join(&path);
            let parent = parent
                .parent()
                .ok_or_else(|| Error::Invalid("source has no parent".into()))?;
            let mut dependencies = Vec::with_capacity(input.dependencies.len());
            for (uri, bytes) in input.dependencies {
                dependencies.push(LocalInput {
                    path: local_path(&root, parent, &uri)?,
                    uri,
                    digest: Digest::of(&bytes),
                });
            }
            let record = SourceDependencies {
                original_path: input.source_path,
                path: path.clone(),
                digest: Digest::of(&input.source),
                inputs: dependencies,
            };
            if inventory.sources.contains_key(&path) {
                return Err(Error::Invalid(format!(
                    "duplicate source dependency snapshot: {path}"
                )));
            }
            inventory.graph.replace(
                path.clone(),
                record.inputs.iter().map(|input| input.path.clone()),
            );
            inventory.sources.insert(path, record);
        }
        Ok(inventory)
    }

    /// Every consumer source, in canonical path order.
    pub fn sources(&self) -> impl Iterator<Item = &SourceDependencies> {
        self.sources.values()
    }

    /// Local URI inputs of one canonical source path; `None` means not inventoried.
    pub fn inputs(&self, source: &str) -> Option<&[LocalInput]> {
        self.sources
            .get(source)
            .map(|source| source.inputs.as_slice())
    }

    /// Direct model/source consumers of a local input; used by safe-delete preflight.
    pub fn users(&self, input: &str) -> impl Iterator<Item = &str> {
        self.graph.users(input)
    }

    /// All transitive consumers, sorted and cycle-safe.
    pub fn invalidated_by(&self, input: &str) -> std::collections::BTreeSet<String> {
        self.graph.invalidated_by(input)
    }

    /// Rejects changed/missing/escaped bytes before committing a prepared mutation.
    /// The caller also revalidates its complete file-state inventory token.
    pub fn verify(&self, root: &Path, cancel: &Cancellation) -> Result<()> {
        let root = root.canonicalize()?;
        for source in self.sources.values() {
            if local_path(&root, &root, &source.original_path)? != source.path {
                return Err(changed(&source.original_path));
            }
            verify_file(&root, &source.path, source.digest, cancel)?;
            let source_file = root.join(&source.path);
            let parent = source_file
                .parent()
                .ok_or_else(|| Error::Invalid("source has no parent".into()))?;
            for input in &source.inputs {
                if local_path(&root, parent, &input.uri)? != input.path {
                    return Err(changed(&input.uri));
                }
                verify_file(&root, &input.path, input.digest, cancel)?;
            }
        }
        Ok(())
    }
}

fn verify_file(root: &Path, path: &str, digest: Digest, cancel: &Cancellation) -> Result<()> {
    cancel.check()?;
    if local_path(root, root, path)? != path
        || Digest::of(&std::fs::read(root.join(path))?) != digest
    {
        return Err(changed(path));
    }
    Ok(())
}

fn changed(path: &str) -> Error {
    Error::Invalid(format!(
        "importer input changed while preparing asset operation: {path}; retry"
    ))
}

fn local_path(root: &Path, base: &Path, uri: &str) -> Result<String> {
    let relative = Path::new(uri);
    if uri.is_empty()
        || uri.contains([':', '\\'])
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
    {
        return Err(Error::Invalid(format!(
            "importer input is not a local relative path: {uri}"
        )));
    }
    let path = base.join(relative).canonicalize()?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::Invalid(format!("importer input escapes authored root: {uri}")))?;
    if !path.is_file() {
        return Err(Error::Invalid(format!(
            "importer input is not a file: {uri}"
        )));
    }
    relative
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or_else(|| Error::Invalid("importer input path is not UTF-8".into()))
}

#[cfg(test)]
mod tests;
