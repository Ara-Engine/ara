//! Exact metadata-only import plan wire shapes.
use super::inventory::one;
use crate::import;
use ara_cmd::builtin::{
    ImportConflictItem, ImportConflictKind, ImportDisposition, ImportEntryKind, ImportPlannedItem,
};
use ara_diag::{Diagnostic, codes};
use ara_doc::AssetId;
pub(super) fn disposition(value: import::Disposition) -> ImportDisposition {
    match value {
        import::Disposition::Created => ImportDisposition::Created,
        import::Disposition::Replaced => ImportDisposition::Replaced,
        import::Disposition::Skipped => ImportDisposition::Skipped,
        import::Disposition::Renamed => ImportDisposition::Renamed,
        import::Disposition::Merged => ImportDisposition::Merged,
    }
}

pub(super) fn conflicts(
    items: &[import::Conflict],
) -> Result<Vec<ImportConflictItem>, Vec<Diagnostic>> {
    items
        .iter()
        .map(|item| {
            Ok(ImportConflictItem {
                source: item
                    .source
                    .to_str()
                    .ok_or_else(|| {
                        one(Diagnostic::error(
                            codes::INVALID_PARAMS,
                            "external source path must be UTF-8",
                        ))
                    })?
                    .to_owned(),
                destination_logical_path: item.destination_logical_path.clone(),
                existing_asset_id: item
                    .existing_asset_id
                    .as_deref()
                    .map(AssetId::parse)
                    .transpose()
                    .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error)))?,
                kind: match item.kind {
                    import::ConflictKind::File => ImportConflictKind::File,
                    import::ConflictKind::Directory => ImportConflictKind::Directory,
                    import::ConflictKind::TypeMismatch => ImportConflictKind::TypeMismatch,
                    import::ConflictKind::Sidecar => ImportConflictKind::Sidecar,
                },
            })
        })
        .collect()
}

pub(super) fn planned(entry: &import::Entry) -> Result<ImportPlannedItem, Vec<Diagnostic>> {
    Ok(ImportPlannedItem {
        source: entry
            .source
            .as_deref()
            .map(|path| {
                path.to_str()
                    .ok_or_else(|| {
                        one(Diagnostic::error(
                            codes::INVALID_PARAMS,
                            "external source path must be UTF-8",
                        ))
                    })
                    .map(str::to_owned)
            })
            .transpose()?,
        destination_logical_path: entry.destination.clone(),
        existing_asset_id: entry
            .asset_id
            .as_deref()
            .map(AssetId::parse)
            .transpose()
            .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error)))?,
        kind: if entry.kind == import::EntryKind::Directory {
            ImportEntryKind::Directory
        } else {
            ImportEntryKind::File
        },
        action: disposition(entry.disposition),
    })
}
