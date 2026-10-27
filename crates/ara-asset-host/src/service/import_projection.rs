//! Seal final source/input proofs against the same native/JSON bytes as commit.
use super::{
    bindings::Bindings,
    inventory::{asset, one},
};
use ara_asset::Cancellation;
use ara_cmd::builtin::{AssetMetadata, ImportDisposition, PreparedAssetImport};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileKind, FileState};
use ara_host::file_state::FileHistory;
use std::path::Path;

pub(super) fn seal(
    root: &Path,
    bindings: &Bindings,
    metadata: &mut AssetMetadata,
    prepared: &mut PreparedAssetImport,
    cancel: &Cancellation,
) -> Result<(), Vec<Diagnostic>> {
    let remap = ara_cmd::builtin::remap_import(
        &bindings.workspace,
        &bindings.schema,
        &metadata.token,
        &prepared.request,
        &prepared.entries,
    )
    .map_err(one)?;
    let history = FileHistory::new(root);
    let mut files = Vec::new();
    for entry in &prepared.entries {
        cancel.check().map_err(asset)?;
        if entry.disposition == ImportDisposition::Skipped
            || entry.state.kind != FileKind::File
            || entry.document.is_some()
            || entry.json.is_some()
        {
            continue;
        }
        let bytes = history.read(&entry.state).map_err(one)?;
        files.push((
            entry.destination.clone(),
            entry.state.clone(),
            ara_asset::Digest::of(&bytes).to_string(),
        ));
    }
    let request = serde_json::to_value(&prepared.request)
        .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
    let key = ara_cmd::builtin::asset_operation_key("asset.import", &request).map_err(one)?;
    let operation = metadata.operations.entry(format!("{key}:0")).or_default();
    for (path, document) in remap.documents {
        cancel.check().map_err(asset)?;
        let bytes = ara_host::project::write_document(&document, &bindings.schema)
            .map_err(one)?
            .into_bytes();
        let state = history.put(&bytes).map_err(one)?;
        let storage = bindings.workspace.storage_path(&path);
        operation
            .document_states
            .insert(storage.clone(), state.clone());
        operation.document_checksums.insert(
            storage,
            FileState::bytes(ara_doc::write(&document, &bindings.schema).as_bytes()).hash,
        );
        files.push((path, state, ara_asset::Digest::of(&bytes).to_string()));
    }
    for blob in remap.json_blobs {
        files.push((
            bindings.workspace.reference_path(&blob.path).to_owned(),
            blob.state,
            ara_asset::Digest::of(&blob.bytes).to_string(),
        ));
    }
    let key = |path: &str| {
        ara_doc::file_state::path_key_with_case(path, prepared.after_inventory.case_insensitive)
    };
    let validated_sources = prepared
        .entries
        .iter()
        .filter(|entry| {
            entry.disposition != ImportDisposition::Skipped
                && prepared
                    .after_inventory
                    .source_paths
                    .iter()
                    .any(|source| key(source) == key(&entry.destination))
        })
        .map(|entry| key(&entry.destination))
        .collect();
    super::import_graph::refresh(&mut prepared.after_inventory, &files, &validated_sources)
}

#[cfg(test)]
mod tests;
