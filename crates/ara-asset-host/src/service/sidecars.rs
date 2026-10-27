//! Source import version transitions are native typed entries in the same batch.
use super::{
    bindings::Bindings,
    import_entries::sidecar_document,
    inventory::{Inventory, one},
};
use ara_cmd::builtin::{ImportDisposition, ImportEntry};
use ara_diag::{Diagnostic, codes};
use std::path::Path;

pub(super) fn prepare(
    root: &Path,
    bindings: &Bindings,
    inventory: &Inventory,
    entries: &mut Vec<ImportEntry>,
    source: &crate::import::PreparedSource,
) -> Result<(), Vec<Diagnostic>> {
    let Some(entry) = provisional(
        bindings,
        inventory,
        entries,
        &source.binding,
        source.foreign_sidecar.as_deref(),
    )?
    else {
        return Ok(());
    };
    let document = entry
        .document
        .as_ref()
        .expect("a prepared sidecar carries its native document");
    let text = ara_host::project::write_document(document, &bindings.schema).map_err(one)?;
    let actual = ara_host::file_state::FileHistory::new(root)
        .put(text.as_bytes())
        .map_err(one)?;
    if actual != entry.state {
        return Err(one(Diagnostic::error(
            codes::STALE_BASE,
            "sidecar preview bytes differ from staged native bytes",
        )));
    }
    upsert(entries, entry);
    Ok(())
}

pub(super) fn upsert(entries: &mut Vec<ImportEntry>, entry: ImportEntry) {
    if let Some(index) = entries
        .iter()
        .position(|existing| existing.destination == entry.destination)
    {
        entries[index] = entry;
    } else {
        entries.push(entry);
    }
}

pub(super) fn provisional(
    bindings: &Bindings,
    inventory: &Inventory,
    entries: &[ImportEntry],
    binding: &ara_asset::SourceAsset,
    foreign_sidecar: Option<&[u8]>,
) -> Result<Option<ImportEntry>, Vec<Diagnostic>> {
    let sidecar = format!("{}.import.json", binding.path);
    let storage = bindings.workspace.storage_path(&sidecar);
    let index = entries
        .iter()
        .position(|entry| entry.destination == sidecar);
    let existing = bindings
        .workspace
        .docs()
        .find_map(|(path, document)| (path == storage).then_some(document));
    let old_version = existing
        .and_then(|document| document.record("i_settings"))
        .and_then(|record| record.get("version"))
        .and_then(serde_json::Value::as_str);
    if existing.is_some() && old_version == Some(binding.version.as_str()) {
        return Ok(None);
    }
    let mut document = if let Some(existing) = existing {
        let mut document = existing.clone();
        // Keep the foreign key for pure remapping, while replacing only the
        // destination's version and retaining its identity/settings at commit.
        if let Some(foreign) = index.and_then(|index| entries[index].document.as_ref()) {
            document.id = foreign.id.clone();
        }
        document
    } else if let Some(incoming) = index.and_then(|index| entries[index].document.as_ref()) {
        incoming.clone()
    } else {
        sidecar_document(binding, foreign_sidecar, bindings)?
    };
    let edit = ara_asset::SidecarCommand::Version(binding.version.clone())
        .edit(document.id.clone())
        .map_err(super::inventory::asset)?;
    document.apply(edit, &bindings.schema).map_err(one)?;
    let text = ara_host::project::write_document(&document, &bindings.schema).map_err(one)?;
    let state = ara_doc::file_state::FileState::bytes(text.as_bytes());
    let source_reference = index
        .map(|index| entries[index].source_reference.clone())
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.destination == binding.path)
                .map(|entry| format!("{}.import.json", entry.source_reference))
        })
        .ok_or_else(|| {
            one(Diagnostic::error(
                codes::INVALID_PARAMS,
                "prepared source has no matching incoming media entry",
            ))
        })?;
    let entry = ImportEntry {
        source_reference,
        destination: sidecar,
        disposition: if existing.is_some() {
            ImportDisposition::Replaced
        } else {
            index.map_or(ImportDisposition::Created, |index| {
                entries[index].disposition
            })
        },
        state,
        previous: inventory.prepared.entries.get(&storage).cloned().flatten(),
        existing_id: existing.map(|document| document.id.clone()),
        document: Some(document),
        json: None,
    };
    Ok(Some(entry))
}
