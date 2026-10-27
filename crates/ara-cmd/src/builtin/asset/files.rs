use super::{AssetBlob, AssetDocument, AssetMetadata, AssetOperationMetadata, invalid, metadata};
use crate::EditCtx;
use ara_diag::Diagnostic;
use ara_doc::file_state::{FileKind, FileState};
use ara_doc::{AssetId, Document};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) type Documents = BTreeMap<AssetId, (String, Document)>;
pub(super) fn documents(ctx: &EditCtx<'_, '_>) -> Documents {
    ctx.read()
        .workspace()
        .docs()
        .map(|(path, document)| (document.id.clone(), (path.to_owned(), document.clone())))
        .collect()
}

pub(super) fn parents(ctx: &mut EditCtx<'_, '_>, path: &str) -> Result<(), Diagnostic> {
    let mut ancestors = Vec::new();
    let mut parent = path.rsplit_once('/').map(|(parent, _)| parent);
    while let Some(path) = parent {
        ancestors.push(path.to_owned());
        parent = path.rsplit_once('/').map(|(parent, _)| parent);
    }
    for path in ancestors.into_iter().rev() {
        match ctx.file_state(&path) {
            Some(state) if state.kind != FileKind::Directory => {
                return Err(invalid(format!("destination parent `{path}` is a file")));
            }
            Some(_) => {}
            None => ctx.set_file(&path, Some(FileState::directory()))?,
        }
    }
    Ok(())
}

pub(super) fn checked_blob(metadata: &AssetMetadata, blob: &AssetBlob) -> Result<(), Diagnostic> {
    if !metadata.preview
        && !metadata
            .json_blobs
            .iter()
            .any(|prepared| prepared.path == blob.path && prepared.state == blob.state)
    {
        return Err(invalid(format!(
            "canonical JSON for `{}` was not stored by preparation; reprepare before commit",
            blob.path
        )));
    }
    Ok(())
}

pub(super) fn json(
    ctx: &mut EditCtx<'_, '_>,
    metadata: &AssetMetadata,
    path: &str,
    value: &Value,
) -> Result<AssetBlob, Diagnostic> {
    let blob = metadata::json_blob(path, value)?;
    checked_blob(metadata, &blob)?;
    parents(ctx, path)?;
    ctx.set_file(path, Some(blob.state.clone()))?;
    Ok(blob)
}

pub(super) fn sync_documents(
    ctx: &mut EditCtx<'_, '_>,
    before: &Documents,
    metadata: &AssetMetadata,
    operation: &AssetOperationMetadata,
    key: &str,
) -> Result<Vec<AssetDocument>, Diagnostic> {
    let current = documents(ctx);
    let mut changed = Vec::new();
    for (id, (path, document)) in current {
        if before
            .get(&id)
            .is_some_and(|(old_path, old)| old_path == &path && old == &document)
        {
            continue;
        }
        let canonical = ara_doc::write(&document, ctx.read().schema());
        let pure = FileState::bytes(canonical.as_bytes());
        if operation
            .document_checksums
            .get(&path)
            .is_some_and(|expected| expected != &pure.hash)
        {
            return Err(invalid(format!(
                "native values for `{path}` changed after preparation; retry"
            )));
        }
        let state = match operation.document_states.get(&path) {
            Some(state) => state.clone(),
            None if metadata.preview => pure.clone(),
            None => {
                return Err(invalid(format!(
                    "native bytes for `{path}` were not prepared with the project codec"
                )));
            }
        };
        if !metadata.preview && operation.document_checksums.get(&path) != Some(&pure.hash) {
            return Err(invalid(format!(
                "native proof for `{path}` is missing; retry preparation"
            )));
        }
        state.validate()?;
        parents(ctx, &path)?;
        ctx.set_file(&path, Some(state))?;
        changed.push(AssetDocument {
            operation_key: key.to_owned(),
            path,
            document,
            pure_hash: pure.hash,
        });
    }
    Ok(changed)
}
