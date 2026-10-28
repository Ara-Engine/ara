//! Exact native/blob identities used by the disposable cook after native remapping.
use super::{
    bindings::{AssetHashes, Bindings},
    inventory::Inventory,
};
use crate::import;
use ara_asset::{Cancellation, Digest, Error, Result};
use ara_cmd::builtin::{ImportEntry, ImportRemap};

pub(super) fn prepare(
    base: &Bindings,
    inventory: &Inventory,
    entries: &[import::Entry],
    decoded: &[ImportEntry],
    remap: &ImportRemap,
    read: &dyn Fn(&import::Entry) -> Result<Vec<u8>>,
    cancel: &Cancellation,
) -> Result<AssetHashes> {
    let mut hashes = base.asset_hashes.clone();
    for entry in entries {
        cancel.check()?;
        if entry.kind != import::EntryKind::File || entry.destination.ends_with(".import.json") {
            continue;
        }
        let bytes = if entry.disposition == import::Disposition::Skipped {
            inventory
                .content(&base.workspace.storage_path(&entry.destination))
                .map_err(failure)?
        } else {
            read(entry)?
        };
        let proof = (entry.destination.clone(), String::new(), Digest::of(&bytes));
        hashes.insert(entry.destination.clone(), proof.clone());
        hashes.insert(entry.source_reference.clone(), proof);
    }
    for (path, document) in &remap.documents {
        cancel.check()?;
        let (asset_path, digest) = if document.kind == "import" {
            let path = path.strip_suffix(".import.json").ok_or_else(|| {
                Error::Invalid("remapped sidecar is not paired with source media".into())
            })?;
            let digest = hashes.get(path).map(|proof| proof.2).ok_or_else(|| {
                Error::Invalid(format!("source media proof is missing for `{path}`"))
            })?;
            (path.to_owned(), digest)
        } else {
            (
                path.clone(),
                Digest::of(
                    ara_host::project::write_document(document, &base.schema)
                        .map_err(|diagnostic| failure(vec![diagnostic]))?
                        .as_bytes(),
                ),
            )
        };
        let proof = (asset_path.clone(), document.id.to_string(), digest);
        hashes.insert(asset_path, proof.clone());
        hashes.insert(document.id.to_string(), proof);
    }
    for (foreign, (id, path)) in &remap.assets {
        let proof = hashes
            .get(id.as_str())
            .or_else(|| hashes.get(path))
            .cloned()
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "remapped dependency `{foreign}` has no exact native/blob proof"
                ))
            })?;
        hashes.insert(foreign.clone(), proof.clone());
        hashes.insert(id.to_string(), proof.clone());
        if let Some(original) = decoded.iter().find(|entry| {
            entry
                .document
                .as_ref()
                .is_some_and(|document| document.id.as_str() == foreign)
        }) {
            let source = if original
                .document
                .as_ref()
                .is_some_and(|document| document.kind == "import")
            {
                original
                    .source_reference
                    .strip_suffix(".import.json")
                    .unwrap_or(&original.source_reference)
            } else {
                &original.source_reference
            };
            hashes.insert(source.to_owned(), proof);
        }
    }
    for blob in &remap.json_blobs {
        let path = base.workspace.reference_path(&blob.path).to_owned();
        hashes.insert(path.clone(), (path, String::new(), Digest::of(&blob.bytes)));
    }
    Ok(hashes)
}

fn failure(diagnostics: Vec<ara_diag::Diagnostic>) -> Error {
    Error::Invalid(
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
