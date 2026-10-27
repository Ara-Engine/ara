//! Typed commit half of an asynchronous OS-file drop. All I/O, discovery/cook and
//! conflict planning completed before the host attaches this immutable snapshot.
use super::{SourceDoc, invalid, references, remove_document};
use crate::{EditCtx, Hints, Job, JobCtx};
use ara_diag::Diagnostic;
use ara_doc::file_state::FileState;
use ara_doc::{AssetId, Document, Edit};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Conflict resolution chosen before the background import preparation begins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportConflict {
    /// Keep existing data and allocate deterministic, compound-extension-safe names.
    #[default]
    Rename,
    /// Replace matching files while preserving existing asset IDs/sidecar settings;
    /// merge directories additively without deleting unmentioned children.
    Replace,
    /// Keep colliding destination entries; a directory collision skips its subtree.
    Skip,
}

/// Parameters of the asynchronous `asset.import` typed job.
#[derive(Clone, Debug, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(params, module = "core")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetImportParams {
    /// Explicit absolute OS files or folder roots authorizing external reads.
    pub sources: Vec<String>,
    /// Authored-relative destination folder; empty means the authored root.
    pub destination: String,
    /// Rename by default; Replace and Skip are explicit conflict policies.
    #[serde(default)]
    #[ara(default = "rename")]
    pub conflict: ImportConflict,
    /// Read-only conflict/source preflight; no staging, cooking or file changes.
    #[serde(default)]
    #[ara(default = false)]
    pub dry_run: bool,
}

/// Imports an externally prepared selection in one reversible typed transaction.
pub struct AssetImport;
impl Job for AssetImport {
    type Params = AssetImportParams;
    type Output = Value;
    const NAME: &'static str = "asset.import";
    const SCOPE: &'static str = "edit:assets";
    const SUMMARY: &'static str = "Import OS files/folders asynchronously with progress, cancellation, explicit conflicts and one undo step.";
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: true,
    };
    fn examples() -> Vec<Value> {
        vec![
            json!({"sources":["C:/Art/crate"],"destination":"props","conflict":"rename"}),
            json!({"sources":["C:/Art/crate"],"destination":"props","dryRun":true}),
        ]
    }
    fn run(ctx: &mut JobCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        let metadata = super::AssetMetadata::read(ctx.edit.prepared_metadata())?;
        let key = metadata.slot(&ctx.edit, &super::operation_key(Self::NAME, &params)?);
        let operation = metadata.operation(&key)?;
        if params.dry_run {
            let prepared = operation.preflight.clone().ok_or_else(|| {
                invalid("asset.import dryRun requires checked read-only preparation")
            })?;
            if prepared.request != params {
                return Err(invalid(
                    "prepared preflight does not match this job's original intent",
                ));
            }
            if prepared.token.is_empty() {
                return Err(invalid("prepared preflight has no checked inventory token"));
            }
            if prepared.token != metadata.token {
                return Err(invalid(
                    "prepared preflight belongs to a different inventory; retry preparation",
                ));
            }
            return Ok(json!({"conflicts":prepared.conflicts,"planned":prepared.planned}));
        }
        let prepared=operation.import.clone().ok_or_else(||invalid("asset.import must be prepared asynchronously by the project host before its typed commit"))?;
        if prepared.request != params {
            return Err(invalid(
                "prepared import does not match this job's original intent",
            ));
        }
        ctx.progress(0.99, "Committing imported assets");
        commit_prepared(&mut ctx.edit, prepared, &metadata, &operation, &key)
    }
}

/// Planned operation copied from background preparation, without OS paths/bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDisposition {
    Created,
    Replaced,
    Skipped,
    Renamed,
    Merged,
}

/// Exact snake_case conflict kind consumed by OS-file-drop clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportConflictKind {
    File,
    Directory,
    TypeMismatch,
    Sidecar,
}

/// A pre-policy collision, retained even after Rename/Replace/Skip resolves it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportConflictItem {
    /// Absolute incoming OS source.
    pub source: String,
    /// Requested authored-relative collision path.
    pub destination_logical_path: String,
    /// Existing asset identity, or null for an ordinary file/directory.
    pub existing_asset_id: Option<AssetId>,
    /// file | directory | type_mismatch | sidecar.
    pub kind: ImportConflictKind,
}

/// One final metadata-only planned entry after the chosen conflict policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlannedItem {
    /// Absolute incoming source, or null for a new destination parent directory.
    pub source: Option<String>,
    /// Final authored-relative path, including any deterministic rename.
    pub destination_logical_path: String,
    /// Existing identity retained by Replace/Skip, otherwise null.
    pub existing_asset_id: Option<AssetId>,
    /// File or directory.
    pub kind: ImportEntryKind,
    /// created | replaced | skipped | renamed | merged.
    pub action: ImportDisposition,
}

/// Filesystem kinds in a read-only plan.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportEntryKind {
    File,
    Directory,
}

/// Read-only worker result attached to the generic prepared metadata channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedImportPreflight {
    /// Exact original request, including dryRun:true.
    pub request: AssetImportParams,
    /// Revalidated complete destination/source/policy token.
    pub token: String,
    /// Actual conflicts before applying the policy.
    pub conflicts: Vec<ImportConflictItem>,
    /// Final metadata-only operations; no content has been staged or cooked.
    pub planned: Vec<ImportPlannedItem>,
}

/// Parsed canonical JSON requiring owner-aware path/ID remapping before staging.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportJson {
    /// Explicit host-recognized JSON owner; never inferred from arbitrary keys.
    pub owner: references::JsonOwner,
    /// Foreign scene identity for UI binding/music entity-address remapping.
    pub context: Option<AssetId>,
    /// Immutable value parsed on the background worker.
    pub value: Value,
}

/// One immutable entry ready for the pure typed commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportEntry {
    /// Original authored-relative reference spelling in the foreign selection.
    pub source_reference: String,
    /// Final authored-relative destination after conflict/companion planning.
    pub destination: String,
    /// Explicit conflict outcome.
    pub disposition: ImportDisposition,
    /// Immutable incoming blob/directory state supplied by the shared file primitive.
    pub state: FileState,
    /// Required existing target state (None means the path must remain absent).
    pub previous: Option<FileState>,
    /// Existing destination identity retained by Replace/Skip.
    pub existing_id: Option<AssetId>,
    /// Native Ara document parsed/validated on the background worker; foreign IDs
    /// are remapped here rather than copying its original header bytes.
    #[serde(with = "super::metadata::optional_document")]
    pub document: Option<Document>,
    /// Known JSON owner parsed by the background host adapter, when applicable.
    pub json: Option<ImportJson>,
}

/// Complete prepared job handle bound to original intent and destination state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedAssetImport {
    /// Exactly the original typed request; a token cannot authorize another import.
    pub request: AssetImportParams,
    /// Shared file/directory/source/sidecar/policy inventory freshness token.
    pub token: String,
    /// Complete deterministic import plan; staged bytes live in the shared blob store.
    pub entries: Vec<ImportEntry>,
    /// Complete recognized-source graph after import publication.
    pub after_inventory: super::AssetInventory,
}

fn commit_prepared(
    ctx: &mut EditCtx<'_, '_>,
    prepared: PreparedAssetImport,
    metadata: &super::AssetMetadata,
    operation: &super::AssetOperationMetadata,
    key: &str,
) -> Result<Value, Diagnostic> {
    if prepared.token.is_empty() {
        return Err(invalid("prepared import has no inventory token"));
    }
    if prepared.token != metadata.token {
        return Err(invalid(
            "prepared import belongs to a different inventory; retry preparation",
        ));
    }
    let root = ctx.read().workspace().reference_root().to_owned();
    let storage = |path: &str| {
        if root.is_empty() {
            path.to_owned()
        } else {
            format!("{root}/{path}")
        }
    };
    let paths = super::projection::file_view(
        ctx.read().workspace(),
        &metadata.token,
        &operation.inventory,
    );
    let mut destinations = BTreeSet::new();
    for entry in &prepared.entries {
        paths
            .editable(&storage(&entry.destination))
            .map_err(|diagnostic| diagnostic.path("destination"))?;
        super::editable(&entry.source_reference)?;
        entry.state.validate()?;
        if !destinations.insert(entry.destination.clone())
            && entry.disposition != ImportDisposition::Skipped
        {
            return Err(invalid("prepared import repeats a writable destination"));
        }
        if ctx.file_state(&storage(&entry.destination)) != entry.previous.as_ref() {
            return Err(invalid(format!(
                "destination `{}` changed during import preparation; retry",
                entry.destination
            )));
        }
        if let Some(document) = &entry.document {
            if document.kind == "project" {
                return Err(invalid(
                    "project metadata cannot be imported as authored content",
                ));
            }
            let kind = ctx
                .read()
                .schema()
                .kind(&document.kind)
                .ok_or_else(|| invalid("unregistered incoming document kind"))?;
            if !entry.destination.ends_with(&format!(".{}", kind.extension)) {
                return Err(invalid(
                    "incoming native document must retain its registered extension",
                ));
            }
            if let Some(id) = &entry.existing_id
                && let Some(existing) = ctx.read().workspace().doc(id.as_str())
                && (ctx.read().workspace().path_of(id.as_str())
                    != Some(storage(&entry.destination).as_str())
                    || existing.kind != document.kind)
            {
                return Err(invalid(
                    "native replacement identity does not match its destination",
                ));
            }
        }
    }
    let remapped = super::remap::remap(
        ctx.read().workspace(),
        ctx.read().schema(),
        &format!("{}:{key}", metadata.token),
        &prepared.entries,
    )?;
    for blob in &remapped.json_blobs {
        super::files::checked_blob(metadata, blob)?;
    }
    let before = super::files::documents(ctx);
    for entry in &prepared.entries {
        if entry.disposition != ImportDisposition::Replaced || entry.document.is_none() {
            continue;
        }
        if let Some(existing) = entry
            .existing_id
            .as_ref()
            .and_then(|id| ctx.read().workspace().doc(id.as_str()).cloned())
        {
            remove_document(
                ctx,
                &SourceDoc {
                    path: storage(&entry.destination),
                    reference_path: entry.destination.clone(),
                    document: existing,
                },
            )?;
        }
    }
    for (path, document) in &remapped.documents {
        ctx.create_document(path, &document.kind, Some(document.id.as_str()))?;
        for (id, record) in document.records() {
            ctx.apply_edit(Edit::Create {
                doc: document.id.clone(),
                id: id.clone(),
                record: record.clone(),
            })?;
        }
    }
    let mut outputs = BTreeMap::<&str, Vec<Value>>::from([
        ("created", Vec::new()),
        ("replaced", Vec::new()),
        ("skipped", Vec::new()),
        ("renamed", Vec::new()),
    ]);
    for entry in &prepared.entries {
        let asset = entry
            .document
            .as_ref()
            .and_then(|document| {
                remapped
                    .assets
                    .get(document.id.as_str())
                    .map(|(id, _)| id.clone())
            })
            .or_else(|| entry.existing_id.clone())
            .or_else(|| {
                prepared
                    .entries
                    .iter()
                    .find(|candidate| {
                        candidate.destination == format!("{}.import.json", entry.destination)
                    })
                    .and_then(|entry| entry.document.as_ref())
                    .and_then(|document| {
                        remapped
                            .assets
                            .get(document.id.as_str())
                            .map(|(id, _)| id.clone())
                    })
            });
        let category = match entry.disposition {
            ImportDisposition::Created => "created",
            ImportDisposition::Replaced => "replaced",
            ImportDisposition::Skipped => "skipped",
            ImportDisposition::Renamed => "renamed",
            ImportDisposition::Merged => continue,
        };
        outputs
            .get_mut(category)
            .expect("known import result category")
            .push(json!({"path":entry.destination,"asset":asset}));
        if entry.disposition == ImportDisposition::Skipped
            || entry.document.is_some()
            || entry.json.is_some()
        {
            continue;
        }
        let path = storage(&entry.destination);
        super::files::parents(ctx, &path)?;
        ctx.set_file(&path, Some(entry.state.clone()))?;
    }
    for blob in &remapped.json_blobs {
        super::files::parents(ctx, &blob.path)?;
        ctx.set_file(&blob.path, Some(blob.state.clone()))?;
    }
    super::files::sync_documents(ctx, &before, metadata, operation, key)?;
    super::projection::file_view(
        ctx.read().workspace(),
        &metadata.token,
        &prepared.after_inventory,
    )
    .validate()?;
    let mut output = json!({"created":outputs["created"],"replaced":outputs["replaced"],"skipped":outputs["skipped"],"renamed":outputs["renamed"]});
    if metadata.preview {
        let mut json_files = operation.json_files.clone();
        for entry in &prepared.entries {
            if entry.disposition == ImportDisposition::Skipped {
                continue;
            }
            if let Some(json) = &entry.json
                && let Some(blob) = remapped
                    .json_blobs
                    .iter()
                    .find(|blob| blob.path == storage(&entry.destination))
            {
                json_files.retain(|file| file.path != blob.path);
                json_files.push(super::AssetJsonFile {
                    path: blob.path.clone(),
                    owner: json.owner,
                    context: json
                        .context
                        .as_ref()
                        .and_then(|old| remapped.assets.get(old.as_str()).map(|(id, _)| id.clone()))
                        .or_else(|| json.context.clone()),
                    value: serde_json::from_slice(&blob.bytes)
                        .map_err(|error| invalid(error.to_string()))?,
                    state: blob.state.clone(),
                });
            }
        }
        output["_assetBlobs"] = serde_json::to_value(&remapped.json_blobs)
            .map_err(|error| invalid(error.to_string()))?;
        output["_assetInventory"] = serde_json::to_value(&prepared.after_inventory)
            .map_err(|error| invalid(error.to_string()))?;
        output["_assetJsonFiles"] =
            serde_json::to_value(json_files).map_err(|error| invalid(error.to_string()))?;
    }
    Ok(output)
}
#[cfg(test)]
mod tests;
