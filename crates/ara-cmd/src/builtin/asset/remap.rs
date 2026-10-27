//! Pure import identity/native/JSON projection, reusable before importer cooking.
use super::{
    AssetBlob, AssetImportParams, ImportDisposition, ImportEntry, SourceDoc, identity, invalid,
    metadata, references,
};
use ara_diag::Diagnostic;
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_schema::SchemaDb;
use std::collections::{BTreeMap, BTreeSet};

mod kept;

/// Exact foreign-to-destination identity and value projection, with no graph or I/O dependency.
pub struct ImportRemap {
    /// Foreign asset identity to concrete destination identity/reference path.
    pub assets: BTreeMap<String, (AssetId, String)>,
    /// Foreign document/record identity to concrete destination record identity.
    pub records: BTreeMap<(String, String), RecordId>,
    /// Remapped native documents at authored-relative destinations; skipped docs are excluded.
    pub documents: Vec<(String, Document)>,
    /// Remapped canonical JSON bytes at physical project storage paths.
    pub json_blobs: Vec<AssetBlob>,
}

/// Project external native identities and references without importer discovery,
/// a mutable ID generator, filesystem access or a complete dependency graph.
pub fn remap_import(
    workspace: &Workspace,
    schema: &SchemaDb,
    token: &str,
    request: &AssetImportParams,
    entries: &[ImportEntry],
) -> Result<ImportRemap, Diagnostic> {
    let key = super::operation_key("asset.import", request)?;
    let identity_token = format!("{token}:{key}:0");
    remap(workspace, schema, &identity_token, entries)
}

pub(super) fn remap(
    workspace: &Workspace,
    schema: &SchemaDb,
    token: &str,
    entries: &[ImportEntry],
) -> Result<ImportRemap, Diagnostic> {
    if token.is_empty() {
        return Err(invalid(
            "native import identity projection requires an operation token",
        ));
    }
    if entries
        .iter()
        .any(|entry| entry.document.is_some() || entry.json.is_some())
    {
        let mut paths = BTreeMap::new();
        for entry in entries
            .iter()
            .filter(|entry| entry.state.kind == ara_doc::file_state::FileKind::File)
        {
            if let Some(previous) = paths.insert(
                ara_doc::file_state::path_key(&entry.source_reference),
                &entry.destination,
            ) && previous != &entry.destination
            {
                return Err(invalid(format!("incoming path `{}` identifies multiple selected files, so native/JSON path references are ambiguous; drop a common containing folder with unique relative paths, or import the selections separately using explicit asset IDs",entry.source_reference)).path("sources"));
            }
        }
    }
    let mut seen = BTreeSet::new();
    let originals: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            entry.document.as_ref().map(|document| SourceDoc {
                path: workspace.storage_path(&entry.destination),
                reference_path: entry.source_reference.clone(),
                document: document.clone(),
            })
        })
        .collect();
    for source in &originals {
        if !seen.insert(source.document.id.to_string()) {
            return Err(invalid(
                "incoming native assets repeat an asset ID; import the selections separately",
            ));
        }
        if !source.document.conflicts.is_empty() {
            return Err(invalid(
                "resolve incoming native document conflicts before importing",
            ));
        }
    }
    let mut maps = references::Mappings::new("", None, &originals);
    for entry in entries {
        maps.relocate(&entry.source_reference, &entry.destination);
    }
    let mut taken_assets: BTreeSet<_> = workspace
        .docs()
        .map(|(_, document)| document.id.to_string())
        .collect();
    let mut taken_records: BTreeSet<_> = workspace
        .docs()
        .flat_map(|(_, document)| document.records().keys().map(ToString::to_string))
        .collect();
    let mut destinations = BTreeMap::new();
    for entry in entries {
        let Some(document) = &entry.document else {
            continue;
        };
        if matches!(
            entry.disposition,
            ImportDisposition::Skipped | ImportDisposition::Merged
        ) {
            continue;
        }
        let prefix = &schema
            .kind(&document.kind)
            .ok_or_else(|| invalid("incoming native document kind is not registered"))?
            .id_prefix;
        let id = if entry.disposition == ImportDisposition::Replaced {
            entry
                .existing_id
                .clone()
                .ok_or_else(|| invalid("native replacement has no destination identity"))?
        } else {
            identity::asset(
                prefix,
                token,
                &entry.destination,
                document.id.as_str(),
                &mut taken_assets,
            )?
        };
        destinations.insert(
            references::asset_path(&entry.destination, &document.kind).to_owned(),
            id.clone(),
        );
        maps.assets.insert(
            document.id.to_string(),
            (
                id,
                references::asset_path(&entry.destination, &document.kind).to_owned(),
            ),
        );
    }
    for entry in entries {
        let Some(document) = &entry.document else {
            continue;
        };
        let path = references::asset_path(&entry.destination, &document.kind);
        if matches!(
            entry.disposition,
            ImportDisposition::Skipped | ImportDisposition::Merged
        ) {
            let id = entry
                .existing_id
                .clone()
                .or_else(|| destinations.get(path).cloned())
                .or_else(|| workspace.find(path).cloned())
                .ok_or_else(|| invalid("skipped native asset has no destination identity"))?;
            maps.assets
                .insert(document.id.to_string(), (id, path.to_owned()));
        }
        let (id, _) = maps
            .assets
            .get(document.id.as_str())
            .ok_or_else(|| invalid("native asset identity projection is incomplete"))?;
        let existing = workspace.doc(id.as_str());
        let addressable = schema.records_addressable(&document.kind);
        for record in document.records().keys() {
            let preserve = entry.disposition == ImportDisposition::Replaced
                && existing.is_some_and(|existing| existing.record(record.as_str()).is_some());
            let skipped = matches!(
                entry.disposition,
                ImportDisposition::Skipped | ImportDisposition::Merged
            );
            let mapped = if !preserve && !skipped && (record.prefix() == "e" || addressable) {
                identity::record(
                    record.prefix(),
                    token,
                    &entry.destination,
                    &format!("{}:{record}", document.id),
                    &mut taken_records,
                )?
            } else {
                record.clone()
            };
            maps.records
                .insert((document.id.to_string(), record.to_string()), mapped);
        }
    }
    let mut documents = Vec::new();
    let mut blobs = Vec::new();
    for entry in entries {
        if matches!(
            entry.disposition,
            ImportDisposition::Skipped | ImportDisposition::Merged
        ) {
            continue;
        }
        if let Some(original) = &entry.document {
            let (id, _) = maps
                .assets
                .get(original.id.as_str())
                .expect("every imported native asset was mapped");
            let mut document = Document::new(&original.kind, id.clone());
            for (old, record) in original.records() {
                let mut record = record.clone();
                maps.rewrite_record(original, &mut record);
                let id = maps
                    .records
                    .get(&(original.id.to_string(), old.to_string()))
                    .expect("every imported native record was mapped");
                document.insert_unchecked(id.clone(), record);
            }
            documents.push((entry.destination.clone(), document));
        } else if let Some(json) = &entry.json {
            let mut value = json.value.clone();
            maps.rewrite_json(json.owner, json.context.as_ref(), &mut value);
            blobs.push(metadata::json_blob(
                &workspace.storage_path(&entry.destination),
                &value,
            )?);
        }
    }
    kept::validate(workspace, schema, entries, &maps, &documents)?;
    Ok(ImportRemap {
        assets: maps.assets,
        records: maps.records,
        documents,
        json_blobs: blobs,
    })
}
