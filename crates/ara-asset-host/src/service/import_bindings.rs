//! Pre-cook native identity planning for references inside one external selection.
use super::{
    bindings::Bindings,
    import_context::{bind_json_contexts, validate_native_ids},
    import_entries::entry_metadata_read,
    inventory::Inventory,
    sidecars,
};
use crate::import::{self, Importers};
use ara_asset::{Cancellation, Error, ImportSettings, Result, SourceAsset};
use ara_cmd::builtin::AssetImportParams;
use std::{collections::BTreeMap, path::PathBuf};

pub(super) struct ImportBindings {
    pub root: PathBuf,
    pub base: Bindings,
    pub inventory: Inventory,
    pub token: String,
    pub request: AssetImportParams,
}

struct Planned {
    base: Bindings,
    sources: BTreeMap<String, SourceAsset>,
}

impl Importers for ImportBindings {
    fn source(&self, path: &str, sidecar: Option<&[u8]>) -> Result<Option<SourceAsset>> {
        self.base.source(path, sidecar)
    }
    fn native_document(&self, path: &str) -> bool {
        self.base.native_document(path)
    }
    fn prepare_plan(
        &self,
        entries: &[import::Entry],
        read: &dyn Fn(&import::Entry) -> Result<Vec<u8>>,
        cancel: &Cancellation,
    ) -> Result<Option<Box<dyn Importers>>> {
        let mut decoded = entries
            .iter()
            .map(|entry| {
                entry_metadata_read(&self.root, &self.base, &self.inventory, entry, true, read)
                    .map_err(failure)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut templates = BTreeMap::new();
        for incoming in entries {
            cancel.check()?;
            if incoming.kind != import::EntryKind::File
                || incoming.disposition == import::Disposition::Skipped
                || incoming.destination.ends_with(".import.json")
            {
                continue;
            }
            let sidecar = format!("{}.import.json", incoming.destination);
            let foreign = entries
                .iter()
                .find(|entry| entry.destination == sidecar)
                .map(read)
                .transpose()?;
            let current = if incoming.disposition == import::Disposition::Replaced {
                self.base.workspace.docs().find_map(|(path, document)| {
                    (path == self.base.workspace.storage_path(&sidecar)).then_some(document)
                })
            } else {
                None
            };
            let description = if let Some(document) = current {
                let record = document.record("i_settings").ok_or_else(|| {
                    Error::Invalid("destination source sidecar has no settings".into())
                })?;
                Some((
                    document.id.to_string(),
                    serde_json::from_value::<ImportSettings>(serde_json::Value::Object(
                        record.clone(),
                    ))?,
                ))
            } else {
                self.base
                    .description(&incoming.destination, foreign.as_deref())?
            };
            let Some((id, settings)) = description else {
                continue;
            };
            let binding = self.base.template(&incoming.destination, id, &settings)?;
            if let Some(entry) = sidecars::provisional(
                &self.base,
                &self.inventory,
                &decoded,
                &binding,
                foreign.as_deref(),
            )
            .map_err(failure)?
            {
                sidecars::upsert(&mut decoded, entry);
            }
            templates.insert(incoming.destination.clone(), (binding, settings));
        }
        validate_native_ids(&decoded).map_err(failure)?;
        bind_json_contexts(&mut decoded).map_err(failure)?;
        decoded.sort_by(|left, right| left.destination.cmp(&right.destination));
        let remap = ara_cmd::builtin::remap_import(
            &self.base.workspace,
            &self.base.schema,
            &self.token,
            &self.request,
            &decoded,
        )
        .map_err(|diagnostic| failure(vec![diagnostic]))?;
        let hashes = super::import_hashes::prepare(
            &self.base,
            &self.inventory,
            entries,
            &decoded,
            &remap,
            read,
            cancel,
        )?;
        let mut sidecar_settings = BTreeMap::new();
        for (path, document) in &remap.documents {
            if document.kind == "import" {
                let path = path.strip_suffix(".import.json").ok_or_else(|| {
                    Error::Invalid("remapped sidecar has no paired media path".into())
                })?;
                let record = document
                    .record("i_settings")
                    .ok_or_else(|| Error::Invalid("remapped sidecar has no settings".into()))?;
                sidecar_settings.insert(
                    path.to_owned(),
                    serde_json::from_value::<ImportSettings>(serde_json::Value::Object(
                        record.clone(),
                    ))?,
                );
            }
        }
        let mut sources = BTreeMap::new();
        for (path, (template, settings)) in templates {
            let settings = sidecar_settings.get(&path).unwrap_or(&settings);
            let binding = self.base.template(&path, template.id, settings)?;
            sources.insert(path, self.base.resolve(binding, settings, &hashes)?);
        }
        Ok(Some(Box::new(Planned {
            base: self.base.clone(),
            sources,
        })))
    }
}

impl Importers for Planned {
    fn source(&self, path: &str, _sidecar: Option<&[u8]>) -> Result<Option<SourceAsset>> {
        Ok(self.sources.get(path).cloned())
    }
    fn native_document(&self, path: &str) -> bool {
        self.base.native_document(path)
    }
    fn rebind(&self, binding: &SourceAsset) -> Result<SourceAsset> {
        self.sources.get(&binding.path).cloned().ok_or_else(|| {
            Error::Invalid(format!(
                "prepared import has no resolved binding for `{}`",
                binding.path
            ))
        })
    }
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

#[cfg(test)]
mod tests;
