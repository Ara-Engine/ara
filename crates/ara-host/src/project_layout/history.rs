//! Plans new immutable history blobs so undo follows the migrated reference base.
use super::{Migration, Rewrite, problem};
use crate::{
    file_state::FileHistory,
    project_layout::{
        self,
        references::{self, Owner},
    },
};
use ara_diag::Diagnostic;
use ara_doc::file_state::{FileKind, FileState};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub(super) struct Planner<'a> {
    migration: &'a Migration,
    mappings: &'a [(String, String)],
    resources: BTreeSet<String>,
    tokens: BTreeMap<(String, String, u64), FileState>,
    writes: BTreeMap<String, Rewrite>,
}

impl<'a> Planner<'a> {
    pub fn new(
        migration: &'a Migration,
        mappings: &'a [(String, String)],
        journal: &str,
    ) -> Result<Self, Diagnostic> {
        let mut planner = Self {
            migration,
            mappings,
            resources: BTreeSet::new(),
            tokens: BTreeMap::new(),
            writes: BTreeMap::new(),
        };
        planner.resource_owner(&project_layout::preferences(&migration.root)?);
        // Historic policy may name a manifest that no longer has its original name.
        for line in journal.lines() {
            let entry: Value = serde_json::from_str(line).map_err(|error| {
                problem(format!("repair the journal before migrating: {error}"))
            })?;
            for (path, value) in snapshots(&entry) {
                if path != crate::project::MARKER && path != project_layout::PREFERENCES {
                    continue;
                }
                let state = state(value, path)?;
                if state.kind != FileKind::File {
                    continue;
                }
                let bytes = planner.read(&state)?;
                if let Ok(value) = ara_json::parse(std::str::from_utf8(&bytes).unwrap_or_default())
                {
                    planner.resource_owner(&value);
                }
            }
        }
        Ok(planner)
    }

    pub fn rewrite_entry(&mut self, entry: &mut Value) -> Result<(), Diagnostic> {
        if let Some(ops) = entry
            .get_mut("txn")
            .and_then(|txn| txn.get_mut("ops"))
            .and_then(Value::as_array_mut)
        {
            for op in ops {
                if op["op"] != "file" {
                    continue;
                }
                let path = op["path"]
                    .as_str()
                    .ok_or_else(|| problem("historical file op needs a storage path"))?
                    .to_owned();
                for key in ["state", "prev"] {
                    if let Some(value) = op.get_mut(key).filter(|value| !value.is_null()) {
                        self.token(&path, value)?;
                    }
                }
            }
        }
        if entry["t"] == "fileFlush" {
            let path = entry["path"]
                .as_str()
                .ok_or_else(|| problem("historical file flush needs a storage path"))?
                .to_owned();
            if let Some(value) = entry.get_mut("state").filter(|value| !value.is_null()) {
                self.token(&path, value)?;
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Vec<Rewrite> {
        self.writes.into_values().collect()
    }

    fn resource_owner(&mut self, value: &Value) {
        if let Some(path) = value.pointer("/ui/resources").and_then(Value::as_str) {
            let path = path
                .trim_start_matches('@')
                .split('#')
                .next()
                .unwrap_or(path);
            self.resources.insert(references::path(path, self.mappings));
        }
    }

    fn token(&mut self, path: &str, value: &mut Value) -> Result<(), Diagnostic> {
        ara_doc::file_state::check_path(path)?;
        let original = state(value, path)?;
        if original.kind == FileKind::Directory {
            return Ok(());
        }
        let key = (path.to_owned(), original.hash.clone(), original.len);
        let rewritten = if let Some(state) = self.tokens.get(&key) {
            state.clone()
        } else {
            let before = self
                .read(&original)
                .map_err(|error| error.file(format!(".ara/file-history/{}", original.hash)))?;
            let after = self.authored_bytes(path, &before)?;
            let rewritten = FileState::bytes(&after);
            if after != before {
                let target = self
                    .migration
                    .root
                    .join(".ara/file-history")
                    .join(&rewritten.hash);
                if target.exists() {
                    if self.read(&rewritten)? != after {
                        return Err(problem(
                            "immutable history blob bytes do not match their hash",
                        ));
                    }
                } else if let Some(planned) = self.writes.get(&rewritten.hash) {
                    if planned.after != after {
                        return Err(problem("conflicting immutable history blob bytes"));
                    }
                } else {
                    self.writes.insert(
                        rewritten.hash.clone(),
                        Rewrite {
                            source: target.clone(),
                            target,
                            before: Vec::new(),
                            after,
                            existed: false,
                        },
                    );
                }
            }
            self.tokens.insert(key, rewritten.clone());
            rewritten
        };
        if rewritten != original {
            *value = serde_json::to_value(rewritten).map_err(|error| problem(error.to_string()))?;
        }
        Ok(())
    }

    fn read(&self, state: &FileState) -> Result<Vec<u8>, Diagnostic> {
        let mut path = self.migration.root.clone();
        for part in [".ara", "file-history", state.hash.as_str()] {
            path.push(part);
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                return Err(
                    problem("migration does not follow links in immutable file history")
                        .file(path.display().to_string()),
                );
            }
        }
        FileHistory::new(&self.migration.root).read(state)
    }

    fn authored_bytes(&self, path: &str, before: &[u8]) -> Result<Vec<u8>, Diagnostic> {
        let extension = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if !matches!(
            extension,
            "json"
                | "scene"
                | "prefab"
                | "animation"
                | "anim"
                | "terrain"
                | "foliage"
                | "imports"
                | "music"
                | "mixer"
                | "material"
                | "materialinstance"
                | "graph"
                | "world"
                | "stories"
                | "ui"
                | "css"
                | "uss"
                | "md"
                | "cs"
                | "toml"
                | "input"
                | "nav"
                | "vfx"
                | "l10n"
                | "look"
                | "gltf"
        ) {
            return Ok(before.to_vec());
        }
        let Ok(text) = std::str::from_utf8(before) else {
            return Ok(before.to_vec());
        };
        if extension == "gltf" {
            return self.gltf_bytes(path, before);
        }
        if let Ok(mut value) = ara_json::parse(text) {
            let logical = path
                .strip_prefix(&format!("{}/", project_layout::AUTHORING_ROOT))
                .map(str::to_owned)
                .unwrap_or_else(|| references::path(path, self.mappings));
            let owner = if path == crate::project::MARKER || path == project_layout::PREFERENCES {
                Owner::Project
            } else if path.ends_with(".export.json") {
                Owner::Export
            } else if logical == "ui/resources.json" || self.resources.contains(&logical) {
                Owner::Resources
            } else {
                Owner::Document
            };
            references::document(&mut value, self.mappings, owner);
            if path == crate::project::MARKER {
                value["layoutVersion"] = project_layout::VERSION.into();
                if let Some(fields) = value.as_object_mut() {
                    for key in project_layout::POLICY_KEYS {
                        fields.remove(*key);
                    }
                }
            }
            if path.ends_with(".export.json") {
                value["content"] = project_layout::AUTHORING_ROOT.into();
            }
            if value == ara_json::parse(text).map_err(|error| problem(error.message))? {
                return Ok(before.to_vec());
            }
            return Ok(ara_json::to_canonical_string(&value).into_bytes());
        }
        if extension == "ui" {
            let (_,id)=crate::ui_document::header(text,path).ok_or_else(||problem(format!("cannot preserve historical UI identity in {path}; repair its markup before migrating")))?;
            let pinned = crate::ui_document::with_identity(text, &id)?;
            let rewritten = references::text(&pinned, self.mappings, true);
            let canonical = ara_ui_document::SourceDocument::parse(
                &rewritten,
                ara_ui_document::DocumentKind::Markup,
            )
            .map_err(|error| problem(error.to_string()))?;
            return Ok(canonical.serialize().as_bytes().to_vec());
        }
        Ok(references::text(text, self.mappings, false).into_bytes())
    }

    fn gltf_bytes(&self, path: &str, before: &[u8]) -> Result<Vec<u8>, Diagnostic> {
        let mut value: Value =
            serde_json::from_slice(before).map_err(|error| problem(error.to_string()))?;
        let source = self.migration.root.join(path);
        let target = self
            .migration
            .root
            .join(self.migration.history_storage_path(path, self.mappings)?);
        let mut moves = self.migration.moves.clone();
        // Deleted companions are absent from the live move list, but undo restores
        // them under the same migrated storage contract as every historical file.
        for group in ["buffers", "images"] {
            for item in value
                .get(group)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(uri) = item.get("uri").and_then(Value::as_str) else {
                    continue;
                };
                if uri.contains(':') || uri.starts_with('/') {
                    continue;
                }
                let input = super::lexical(&source.parent().expect("source has parent").join(uri))?;
                let relative = input
                    .strip_prefix(&self.migration.root)
                    .map_err(|_| problem("historical source companion escapes the project"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let output = self.migration.root.join(
                    self.migration
                        .history_storage_path(&relative, self.mappings)?,
                );
                moves.push((input, output));
            }
        }
        if super::rewrite_gltf(&mut value, &source, &target, &moves)? {
            Ok(ara_json::to_canonical_string(&value).into_bytes())
        } else {
            Ok(before.to_vec())
        }
    }
}

fn state(value: &Value, path: &str) -> Result<FileState, Diagnostic> {
    let state: FileState = serde_json::from_value(value.clone())
        .map_err(|error| problem(format!("invalid historical file state for {path}: {error}")))?;
    state.validate().map_err(|error| error.file(path))?;
    Ok(state)
}

fn snapshots(entry: &Value) -> Vec<(&str, &Value)> {
    let mut out = Vec::new();
    if let Some(ops) = entry.pointer("/txn/ops").and_then(Value::as_array) {
        for op in ops {
            if op["op"] == "file"
                && let Some(path) = op["path"].as_str()
            {
                for key in ["state", "prev"] {
                    if let Some(value) = op.get(key).filter(|value| !value.is_null()) {
                        out.push((path, value));
                    }
                }
            }
        }
    }
    if entry["t"] == "fileFlush"
        && let Some(path) = entry["path"].as_str()
        && let Some(value) = entry.get("state").filter(|value| !value.is_null())
    {
        out.push((path, value));
    }
    out
}

#[cfg(test)]
#[path = "history/tests.rs"]
mod tests;
