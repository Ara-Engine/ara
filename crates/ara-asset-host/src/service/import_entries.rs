//! Decode exact native/JSON import bytes in both readonly and staged preparation.
use super::{
    bindings::Bindings,
    inventory::{Inventory, asset, one},
    metadata,
};
use crate::import;
use ara_asset::Cancellation;
use ara_cmd::builtin::{ImportEntry, ImportJson};
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Document, file_state::FileState};
use std::path::Path;
pub(super) fn sidecar_document(
    binding: &ara_asset::SourceAsset,
    foreign_sidecar: Option<&[u8]>,
    bindings: &Bindings,
) -> Result<Document, Vec<Diagnostic>> {
    if let Some(bytes) = foreign_sidecar {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
        return ara_asset::Sidecar::read(
            text,
            &format!("{}.import.json", binding.path),
            &bindings.schema,
        )
        .map(ara_asset::Sidecar::into_document)
        .map_err(asset);
    }
    let id = AssetId::parse(&binding.id)
        .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error)))?;
    let mut document = Document::new("import", id.clone());
    let edit = ara_asset::SidecarCommand::InitializeInherited(ara_asset::ImportSettings {
        importer: binding.importer.clone(),
        version: binding.version.clone(),
        settings: binding.settings.clone(),
        preset: None,
        dependencies: Vec::new(),
    })
    .edit(id)
    .map_err(asset)?;
    document.apply(edit, &bindings.schema).map_err(one)?;
    Ok(document)
}

pub(super) fn entry_metadata(
    root: &Path,
    bindings: &Bindings,
    inventory: &Inventory,
    entry: &import::Entry,
    readonly: bool,
    cancel: &Cancellation,
) -> Result<ImportEntry, Vec<Diagnostic>> {
    entry_metadata_read(root, bindings, inventory, entry, readonly, &|entry| {
        import::read_entry(entry, cancel)
    })
}

pub(super) fn entry_metadata_read(
    root: &Path,
    bindings: &Bindings,
    inventory: &Inventory,
    entry: &import::Entry,
    readonly: bool,
    read: &dyn Fn(&import::Entry) -> ara_asset::Result<Vec<u8>>,
) -> Result<ImportEntry, Vec<Diagnostic>> {
    let storage = bindings.workspace.storage_path(&entry.destination);
    let previous = inventory.prepared.entries.get(&storage).cloned().flatten();
    let existing_id = entry
        .asset_id
        .as_deref()
        .map(AssetId::parse)
        .transpose()
        .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error)))?;
    let disposition = super::import_plan::disposition(entry.disposition);
    let mut document = None;
    let mut json = None;
    let state = if entry.kind == import::EntryKind::Directory {
        FileState::directory()
    } else {
        let bytes = read(entry).map_err(asset)?;
        if entry.native_document {
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string()))
            })?;
            let parsed = ara_host::project::read_document(text, &storage, &bindings.schema, None);
            let errors: Vec<_> = parsed
                .diags
                .into_iter()
                .filter(Diagnostic::is_error)
                .collect();
            if !errors.is_empty() {
                return Err(errors);
            }
            document = Some(parsed.doc.ok_or_else(|| {
                one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "incoming native document has no valid document",
                ))
            })?);
        } else if let Some(owner) = imported_json_owner(&storage, entry, bindings)? {
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string()))
            })?;
            let value = ara_json::parse(text)
                .map_err(|error| one(Diagnostic::error(codes::JSON_SYNTAX, error.message)))?;
            if owner != ara_cmd::builtin::JsonOwner::Generic
                || metadata::has_explicit_reference(&value)
            {
                json = Some(ImportJson {
                    owner,
                    context: None,
                    value,
                });
            }
        }
        if entry.disposition == import::Disposition::Skipped {
            previous.clone().ok_or_else(|| {
                one(Diagnostic::error(
                    codes::STALE_BASE,
                    "skipped import file has no existing destination snapshot",
                ))
            })?
        } else if readonly {
            FileState::bytes(&bytes)
        } else {
            ara_host::file_state::FileHistory::new(root)
                .put(&bytes)
                .map_err(one)?
        }
    };
    Ok(ImportEntry {
        source_reference: entry.source_reference.clone(),
        destination: entry.destination.clone(),
        disposition,
        state,
        previous,
        existing_id,
        document,
        json,
    })
}

fn imported_json_owner(
    storage: &str,
    entry: &import::Entry,
    bindings: &Bindings,
) -> Result<Option<ara_cmd::builtin::JsonOwner>, Vec<Diagnostic>> {
    use ara_cmd::builtin::JsonOwner;
    let destination = metadata::json_owner(storage, bindings)?;
    if destination.is_some_and(|owner| owner != JsonOwner::Generic) {
        return Ok(destination);
    }
    let original = metadata::json_owner(
        &bindings.workspace.storage_path(&entry.source_reference),
        bindings,
    )?;
    if original.is_some_and(|owner| owner != JsonOwner::Generic) {
        return Ok(original);
    }
    if entry.source_reference.ends_with("ui/resources.json") {
        Ok(Some(JsonOwner::UiResources))
    } else if entry.source_reference.ends_with("ui/bindings.json") {
        Ok(Some(JsonOwner::UiBindings))
    } else {
        Ok(destination.or(original))
    }
}
