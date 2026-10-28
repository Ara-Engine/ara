//! Serializable owner facts carried by the shared prepared-file inventory.
use super::{JsonOwner, PreparedAssetImport, PreparedImportPreflight, invalid};
use ara_diag::Diagnostic;
use ara_doc::file_state::FileState;
use ara_doc::{AssetId, Document};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Local URI proof; SHA file-state and BLAKE3 importer identities stay separate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetLocalInput {
    /// Importer-decoded local URI key.
    pub uri: String,
    /// Canonical authored-relative input path.
    pub path: String,
    /// Shared SHA-256 snapshot token for the exact input bytes.
    pub state: FileState,
    /// Importer's BLAKE3 input identity, independently verified on the worker.
    pub blake3: String,
}
/// Complete source consumer proof, including unimported recognized sources.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetConsumer {
    /// Shared SHA-256 source snapshot token.
    pub state: FileState,
    /// Independently verified BLAKE3 source identity.
    pub blake3: String,
    /// Every discovered local URI input.
    pub inputs: Vec<AssetLocalInput>,
}
/// Complete recognized-source graph; arbitrary raw files remain file facts only.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInventory {
    /// True only after all current recognized sources were discovered successfully.
    pub complete: bool,
    /// Destination filesystem case behavior.
    pub case_insensitive: bool,
    /// Exact recognized source set, using authored-relative paths.
    pub source_paths: BTreeSet<String>,
    /// One consumer result per source, keyed by authored-relative source path.
    pub consumers: BTreeMap<String, AssetConsumer>,
}
/// Host-recognized canonical JSON owner requiring path/entity repair.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetJsonFile {
    /// Physical project-relative storage path.
    pub path: String,
    /// Explicit JSON owner, never guessed from arbitrary field names.
    pub owner: JsonOwner,
    /// Original scene identity for binding/music entity remapping.
    pub context: Option<AssetId>,
    /// Worker-parsed immutable value.
    pub value: Value,
    /// Original shared SHA snapshot token.
    pub state: FileState,
}
/// Canonical bytes produced by pure preview, to be stored by the preparation worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetBlob {
    /// Physical project-relative destination.
    pub path: String,
    /// SHA-256 identity of exactly `bytes`.
    pub state: FileState,
    /// Canonical bytes; the command never persists them itself.
    pub bytes: Vec<u8>,
}
/// Facts for one command in an atomic asset batch.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetOperationMetadata {
    /// Complete source inventory at this point in the projected sequence.
    pub inventory: AssetInventory,
    /// JSON owners at this point in the projected sequence.
    pub json_files: Vec<AssetJsonFile>,
    /// Prepared external import, when this command is asset.import.
    pub import: Option<PreparedAssetImport>,
    /// Read-only external import preflight, when requested.
    pub preflight: Option<PreparedImportPreflight>,
    /// Host-codec SHA states for changed native documents.
    pub document_states: BTreeMap<String, FileState>,
    /// Pure canonical document checksums binding each host-codec state to its values.
    pub document_checksums: BTreeMap<String, String>,
}
/// Owner payload placed directly in PreparedInventory.metadata.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetMetadata {
    /// Exactly ara.asset/1.
    pub format: String,
    /// Unique operation/batch token bound to frozen state, policy and request.
    pub token: String,
    /// Initial complete recognized-source inventory.
    pub inventory: AssetInventory,
    /// Initial host-recognized JSON owners.
    pub json_files: Vec<AssetJsonFile>,
    /// Optional external import preparation.
    pub import: Option<PreparedAssetImport>,
    /// Optional read-only import preflight.
    pub preflight: Option<PreparedImportPreflight>,
    /// Internal pure-preview mode; only a temporary cloned store may use it.
    pub preview: bool,
    /// Cumulative prior operation count while preview uses one private transaction per call.
    #[serde(default)]
    pub preview_base_ops: usize,
    /// Exact canonical JSON blobs pre-stored by the preparation worker.
    pub json_blobs: Vec<AssetBlob>,
    /// Host-codec states for a singleton command before per-command projection.
    pub document_states: BTreeMap<String, FileState>,
    /// Pure document checksums for the singleton command.
    pub document_checksums: BTreeMap<String, String>,
    /// Projected per-command facts, keyed by asset_operation_key.
    pub operations: BTreeMap<String, AssetOperationMetadata>,
}
impl AssetMetadata {
    /// Construct a fail-closed owner envelope; set inventory.complete after discovery.
    pub fn new(token: String) -> Self {
        Self {
            format: "ara.asset/1".into(),
            token,
            ..Self::default()
        }
    }
    pub(super) fn read(value: Option<&Value>) -> Result<Self, Diagnostic> {
        let value = value
            .ok_or_else(|| invalid("asset operation requires checked background preparation"))?;
        let metadata: Self = serde_json::from_value(value.clone())
            .map_err(|error| invalid(format!("invalid asset preparation metadata: {error}")))?;
        if metadata.format != "ara.asset/1" || metadata.token.is_empty() {
            return Err(invalid("asset preparation has no valid owner/token"));
        }
        Ok(metadata)
    }
    pub(super) fn operation(&self, key: &str) -> Result<AssetOperationMetadata, Diagnostic> {
        if let Some(operation) = self.operations.get(key) {
            return Ok(operation.clone());
        }
        if !self.operations.is_empty() {
            return Err(invalid(
                "asset intent or staged operation offset differs from its checked batch; retry preparation",
            ));
        }
        Ok(AssetOperationMetadata {
            inventory: self.inventory.clone(),
            json_files: self.json_files.clone(),
            import: self.import.clone(),
            preflight: self.preflight.clone(),
            document_states: self.document_states.clone(),
            document_checksums: self.document_checksums.clone(),
        })
    }
    pub(super) fn slot(&self, ctx: &crate::EditCtx<'_, '_>, key: &str) -> String {
        format!(
            "{key}:{}",
            if self.preview {
                self.preview_base_ops
            } else {
                ctx.staged.ops().len()
            }
        )
    }
}

/// Native document returned by pure preview for host-format serialization on a worker.
#[derive(Clone, Debug)]
pub struct AssetDocument {
    /// Canonical command key owning this intermediate document state.
    pub operation_key: String,
    /// Physical project-relative destination.
    pub path: String,
    /// Exact resulting native values/identities; use the host codec to serialize.
    pub document: Document,
    /// Canonical Ara document checksum binding the supplied host-codec token.
    pub pure_hash: String,
}
/// Pure preview result; no bytes were written and the original workspace is unchanged.
#[derive(Clone, Debug, Default)]
pub struct AssetPreview {
    /// Canonical JSON blobs to FileHistory.put before checked commit.
    pub blobs: Vec<AssetBlob>,
    /// Native documents to serialize with the host codec and FileHistory.put.
    pub documents: Vec<AssetDocument>,
}

pub(super) fn json_blob(path: &str, value: &Value) -> Result<AssetBlob, Diagnostic> {
    ara_doc::file_state::check_path(path)?;
    let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(value)).into_bytes();
    Ok(AssetBlob {
        path: path.to_owned(),
        state: FileState::bytes(&bytes),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_batch_never_reuses_base_facts_for_a_different_call_or_offset() {
        let mut metadata = AssetMetadata::new("checked-batch".into());
        metadata.inventory.complete = true;
        metadata
            .operations
            .insert("first-call:0".into(), AssetOperationMetadata::default());
        assert!(metadata.operation("first-call:0").is_ok());
        for key in ["different-call:0", "first-call:1"] {
            assert!(
                metadata
                    .operation(key)
                    .expect_err("unprepared intent")
                    .message
                    .contains("checked batch")
            );
        }
    }
}

/// Canonical owner key for one typed asset intent, normalizing omitted defaults.
pub fn asset_operation_key(command: &str, params: &Value) -> Result<String, Diagnostic> {
    fn normalize<T: serde::de::DeserializeOwned + Serialize>(
        params: &Value,
    ) -> Result<Value, Diagnostic> {
        let value: T =
            serde_json::from_value(params.clone()).map_err(|error| invalid(error.to_string()))?;
        serde_json::to_value(value).map_err(|error| invalid(error.to_string()))
    }
    let normalized = match command {
        "asset.move" | "asset.duplicate" => normalize::<super::AssetDestinationParams>(params)?,
        "asset.rename" => normalize::<super::AssetRenameParams>(params)?,
        "asset.delete" => normalize::<super::AssetDeleteParams>(params)?,
        "asset.import" => normalize::<super::AssetImportParams>(params)?,
        "folder.create" => normalize::<super::FolderCreateParams>(params)?,
        _ => return Err(invalid("pure asset preview accepts asset commands only")),
    };
    let value = serde_json::json!([command, normalized]);
    Ok(
        FileState::bytes(ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes())
            .hash,
    )
}

pub(super) mod optional_document {
    use super::*;
    use ara_doc::{Record, RecordId};
    use serde::{Deserializer, Serializer};
    #[derive(Serialize, Deserialize)]
    struct Data {
        kind: String,
        id: AssetId,
        records: BTreeMap<RecordId, Record>,
        conflicts: Vec<Value>,
    }
    pub fn serialize<S: Serializer>(
        document: &Option<Document>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        document
            .as_ref()
            .map(|document| Data {
                kind: document.kind.clone(),
                id: document.id.clone(),
                records: document.records().clone(),
                conflicts: document.conflicts.clone(),
            })
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Document>, D::Error> {
        let data = Option::<Data>::deserialize(deserializer)?;
        Ok(data.map(|data| {
            let mut document = Document::new(data.kind, data.id);
            for (id, record) in data.records {
                document.insert_unchecked(id, record);
            }
            document.conflicts = data.conflicts;
            document
        }))
    }
}
