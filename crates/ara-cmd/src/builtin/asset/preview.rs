//! One frozen, no-I/O projection for an atomic asset batch.
use super::*;
use crate::{Batch, Registry, Session};
use ara_doc::file_state::PreparedInventory;
use ara_doc::{IdGen, Workspace};
use ara_schema::SchemaDb;
use std::collections::BTreeSet;

/// Project a complete asset/folder batch on a private workspace. The caller stores
/// returned JSON/native bytes on its worker, then checks/commits the original one
/// batch. `inventory` retains original physical proofs plus all generated absences.
/// Host-codec states supplied after pass one are reused by the second pure pass.
pub fn preview_assets(
    workspace: &Workspace,
    schema: &SchemaDb,
    batch: &Batch,
    inventory: &mut PreparedInventory,
) -> Result<AssetPreview, Vec<Diagnostic>> {
    inventory
        .validate()
        .map_err(|diagnostic| vec![diagnostic])?;
    let mut metadata =
        AssetMetadata::read(Some(&inventory.metadata)).map_err(|diagnostic| vec![diagnostic])?;
    let imports: Vec<_> = batch
        .calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.command == "asset.import")
        .map(|(index, _)| index)
        .collect();
    if imports.len() > 1 {
        return Err(vec![invalid("each external import is one asynchronous job; combine its sources into one asset.import request").command(imports[1])]);
    }
    let mut db = schema.clone();
    let registry = Registry::builtin(&mut db);
    let mut store = ara_ops::Store::with_ids(db, workspace.clone(), IdGen::seeded(0));
    store.clear_prepared_files();
    for (path, state) in &inventory.entries {
        store.observe_file(path.clone(), state.clone());
    }
    let mut graph = metadata.inventory.clone();
    let mut json_files = metadata.json_files.clone();
    let mut preview = AssetPreview::default();
    let mut failures = Vec::new();
    let mut offset = 0usize;
    let prior_operations = metadata.operations.clone();
    metadata.operations.clear();
    metadata.preview = true;
    for (index, call) in batch.calls.iter().enumerate() {
        let result = (|| -> Result<(), Diagnostic> {
            let key = asset_operation_key(&call.command, &call.params)?;
            let slot = format!("{key}:{offset}");
            let previous = prior_operations.get(&slot);
            let mut operation = AssetOperationMetadata {
                inventory: graph.clone(),
                json_files: json_files.clone(),
                import: metadata.import.clone(),
                preflight: metadata.preflight.clone(),
                document_states: previous
                    .map(|operation| operation.document_states.clone())
                    .unwrap_or_default(),
                document_checksums: previous
                    .map(|operation| operation.document_checksums.clone())
                    .unwrap_or_default(),
            };
            metadata.preview_base_ops = offset;
            metadata.operations.insert(slot.clone(), operation.clone());
            let targets = targets(store.workspace(), &metadata, &operation, call)?;
            for path in targets {
                if ara_doc::file_state::check_path(&path).is_ok() {
                    inventory
                        .entries
                        .entry(path.clone())
                        .or_insert_with(|| workspace.file_state(&path).cloned());
                }
            }
            let mut active = inventory.clone();
            for (path, state) in store.workspace().file_entries() {
                active.entries.insert(path.to_owned(), Some(state.clone()));
            }
            for (path, state) in &mut active.entries {
                if store.workspace().file_state(path).is_none() {
                    *state = None;
                }
            }
            active.metadata =
                serde_json::to_value(&metadata).map_err(|error| invalid(error.to_string()))?;
            store.prepare_files(active)?;
            let before: files::Documents = store
                .workspace()
                .docs()
                .map(|(path, document)| (document.id.clone(), (path.to_owned(), document.clone())))
                .collect();
            let mut single = batch.clone();
            single.calls = vec![call.clone()];
            single.base = None;
            single.idempotency_key = None;
            single.dry_run = false;
            let result = match registry.apply(&mut store, &single, &Session::human()) {
                Ok(result) => result,
                Err(errors) => {
                    failures.extend(
                        errors
                            .into_iter()
                            .map(|diagnostic| diagnostic.command(index)),
                    );
                    return Ok(());
                }
            };
            offset = offset
                .checked_add(result.ops.len())
                .ok_or_else(|| invalid("asset operation count overflow"))?;
            for output in &result.outputs {
                if let Some(value) = output.get("_assetBlobs") {
                    let blobs: Vec<AssetBlob> = serde_json::from_value(value.clone())
                        .map_err(|error| invalid(error.to_string()))?;
                    for blob in blobs {
                        if !metadata.json_blobs.iter().any(|existing| {
                            existing.path == blob.path && existing.state == blob.state
                        }) {
                            metadata.json_blobs.push(blob.clone());
                        }
                        preview.blobs.push(blob);
                    }
                }
                if let Some(value) = output.get("_assetInventory") {
                    graph = serde_json::from_value(value.clone())
                        .map_err(|error| invalid(error.to_string()))?;
                }
                if let Some(value) = output.get("_assetJsonFiles") {
                    json_files = serde_json::from_value(value.clone())
                        .map_err(|error| invalid(error.to_string()))?;
                }
            }
            for (path, document) in store.workspace().docs() {
                if before
                    .get(&document.id)
                    .is_some_and(|(old_path, old)| old_path == path && old == document)
                {
                    continue;
                }
                let hash =
                    FileState::bytes(ara_doc::write(document, store.schema()).as_bytes()).hash;
                operation
                    .document_checksums
                    .insert(path.to_owned(), hash.clone());
                preview.documents.push(AssetDocument {
                    operation_key: slot.clone(),
                    path: path.to_owned(),
                    document: document.clone(),
                    pure_hash: hash,
                });
            }
            metadata.operations.insert(slot, operation);
            Ok(())
        })();
        if let Err(diagnostic) = result {
            failures.push(diagnostic.command(index));
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    metadata.preview = false;
    metadata.preview_base_ops = 0;
    inventory.metadata =
        serde_json::to_value(metadata).map_err(|error| vec![invalid(error.to_string())])?;
    inventory
        .validate()
        .map_err(|diagnostic| vec![diagnostic])?;
    Ok(preview)
}

fn targets(
    workspace: &Workspace,
    metadata: &AssetMetadata,
    operation: &AssetOperationMetadata,
    call: &crate::Call,
) -> Result<BTreeSet<String>, Diagnostic> {
    let root = workspace.reference_root();
    let storage = |path: &str| {
        if root.is_empty() {
            path.to_owned()
        } else {
            format!("{root}/{path}")
        }
    };
    let mut paths: BTreeSet<_> = workspace.docs().map(|(path, _)| path.to_owned()).collect();
    match call.command.as_str() {
        "asset.move" | "asset.duplicate" | "asset.rename" => {
            let (source, dest, mode) = if call.command == "asset.rename" {
                let p: AssetRenameParams = serde_json::from_value(call.params.clone())
                    .map_err(|error| invalid(error.to_string()))?;
                let dest = Path::new(&p.source)
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(p.name)
                    .to_string_lossy()
                    .replace('\\', "/");
                (p.source, dest, Mode::Move)
            } else {
                let p: AssetDestinationParams = serde_json::from_value(call.params.clone())
                    .map_err(|error| invalid(error.to_string()))?;
                (
                    p.source,
                    p.destination,
                    if call.command == "asset.duplicate" {
                        Mode::Duplicate
                    } else {
                        Mode::Move
                    },
                )
            };
            let source = storage(&source);
            let dest = storage(&dest);
            for (path, _) in workspace
                .file_entries()
                .filter(|(path, _)| selected(path, &source))
            {
                paths.insert(destination(path, &source, &dest));
            }
            for (path, _) in workspace.docs().filter(|(path, _)| selected(path, &source)) {
                paths.insert(destination(path, &source, &dest));
            }
            let view = projection::file_view(workspace, &metadata.token, &operation.inventory);
            for (path, _) in dependencies::prepare(&view, &source, Some(&dest), mode)? {
                paths.insert(path);
            }
        }
        "asset.delete" => {}
        "folder.create" => {
            let p: FolderCreateParams = serde_json::from_value(call.params.clone())
                .map_err(|error| invalid(error.to_string()))?;
            paths.insert(storage(&p.path));
        }
        "asset.import" => {
            if let Some(import) = &operation.import {
                for entry in &import.entries {
                    paths.insert(storage(&entry.destination));
                }
            }
        }
        _ => {
            return Err(invalid(
                "pure asset preview accepts asset and folder commands only",
            ));
        }
    }
    let leaves: Vec<_> = paths.iter().cloned().collect();
    for path in leaves {
        let mut parent = path.rsplit_once('/').map(|(parent, _)| parent.to_owned());
        while let Some(path) = parent {
            paths.insert(path.clone());
            parent = path.rsplit_once('/').map(|(parent, _)| parent.to_owned());
        }
    }
    Ok(paths)
}
