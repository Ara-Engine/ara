//! Converts staged imports into pure command metadata; never publishes authored files.
use super::{
    SpaceCheck,
    bindings::Bindings,
    import_context::{bind_json_contexts, validate_native_ids},
    import_entries::entry_metadata,
    import_plan::{conflicts, planned},
    inventory::{Inventory, asset},
};
use crate::import;
use ara_asset::{CacheBackend, Cancellation, ProcessWorker};
use ara_cmd::builtin::{
    AssetImportParams, AssetMetadata, PreparedAssetImport, PreparedImportPreflight,
};
use ara_diag::Diagnostic;
use ara_doc::file_state::FileKind;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Context<'a> {
    pub root: &'a Path,
    pub bindings: &'a Bindings,
    pub inventory: &'a Inventory,
    pub worker: &'a ProcessWorker,
    pub check_space: &'a SpaceCheck,
    pub cancel: &'a Cancellation,
    pub progress: &'a mut dyn FnMut(f32, &str),
}

pub(super) fn prepare(
    context: Context<'_>,
    metadata: &mut AssetMetadata,
    request: AssetImportParams,
) -> Result<(), Vec<Diagnostic>> {
    let Context {
        root,
        bindings,
        inventory,
        worker,
        check_space,
        cancel,
        progress,
    } = context;
    let mut entries = BTreeMap::new();
    for (storage, state) in &inventory.prepared.entries {
        let Some(state) = state else {
            continue;
        };
        let reference = bindings.workspace.reference_path(storage);
        if reference == storage && !bindings.workspace.reference_root().is_empty() {
            continue;
        }
        let directory = state.kind == FileKind::Directory;
        let bytes = if directory {
            None
        } else {
            Some(inventory.content(storage)?)
        };
        let binding = if directory {
            None
        } else {
            bindings.existing(reference).map_err(asset)?
        };
        entries.insert(
            reference.to_owned(),
            import::ExistingEntry {
                kind: if directory {
                    import::EntryKind::Directory
                } else {
                    import::EntryKind::File
                },
                bytes: state.len,
                digest: bytes.as_deref().map(ara_asset::Digest::of),
                asset_id: bindings
                    .workspace
                    .find(storage)
                    .or_else(|| bindings.workspace.find(reference))
                    .map(ToString::to_string),
                binding,
            },
        );
    }
    let destination = import::DestinationInventory {
        token: metadata.token.clone(),
        case_insensitive: metadata.inventory.case_insensitive,
        entries,
    };
    let authored = root.join(bindings.workspace.reference_root());
    let staging = root.join(".ara").join(format!(
        "import-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let importers = super::import_bindings::ImportBindings {
        root: root.to_owned(),
        base: bindings.clone(),
        inventory: inventory.clone(),
        token: metadata.token.clone(),
        request: request.clone(),
    };
    let outcome = import::run(
        import::Request {
            sources: request
                .sources
                .iter()
                .map(std::path::PathBuf::from)
                .collect(),
            destination: request.destination.clone(),
            conflict: match request.conflict {
                ara_cmd::builtin::ImportConflict::Rename => import::ImportConflict::Rename,
                ara_cmd::builtin::ImportConflict::Replace => import::ImportConflict::Replace,
                ara_cmd::builtin::ImportConflict::Skip => import::ImportConflict::Skip,
            },
            dry_run: request.dry_run,
        },
        import::Context {
            project_root: root,
            authored_root: &authored,
            staging: &staging,
            inventory: &destination,
            worker,
            importers: &importers,
            limits: import::Limits {
                bytes: 256 * 1024 * 1024 * 1024,
                entries: 1_000_000,
            },
            check_space,
        },
        cancel,
        &mut |update| {
            let fraction = if update.total == 0 {
                0.0
            } else {
                update.received as f32 / update.total as f32
            };
            let phase = match update.phase {
                import::Phase::Inspecting => "Inspecting",
                import::Phase::Staging => "Copying",
                import::Phase::Discovering => "Discovering dependencies for",
                import::Phase::Cooking => "Validating",
                import::Phase::Ready => "Prepared",
            };
            progress(
                0.45 + fraction * 0.35,
                &format!(
                    "{phase} {}",
                    update.path.as_deref().unwrap_or("import selection")
                ),
            );
        },
    )
    .map_err(asset)?;
    match outcome {
        import::Outcome::Preflight(plan) => {
            let mut decoded = plan
                .planned
                .iter()
                .map(|entry| entry_metadata(root, bindings, inventory, entry, true, cancel))
                .collect::<Result<Vec<_>, _>>()?;
            validate_native_ids(&decoded)?;
            bind_json_contexts(&mut decoded)?;
            metadata.preflight = Some(PreparedImportPreflight {
                request,
                token: plan.token,
                conflicts: conflicts(&plan.conflicts)?,
                planned: plan.planned.iter().map(planned).collect::<Result<_, _>>()?,
            });
        }
        import::Outcome::Ready(prepared) => {
            prepared.verify(cancel).map_err(asset)?;
            let mut entries = prepared
                .entries
                .iter()
                .map(|entry| entry_metadata(root, bindings, inventory, entry, false, cancel))
                .collect::<Result<Vec<_>, _>>()?;
            validate_native_ids(&entries)?;
            let cache = ara_asset::FileCache::new(root).map_err(asset)?;
            for source in &prepared.sources {
                cancel.check().map_err(asset)?;
                cache
                    .store(source.input.key().map_err(asset)?, &source.output)
                    .map_err(asset)?;
                super::sidecars::prepare(root, bindings, inventory, &mut entries, source)?;
            }
            prepared.verify(cancel).map_err(asset)?;
            bind_json_contexts(&mut entries)?;
            entries.sort_by(|left, right| left.destination.cmp(&right.destination));
            let after_inventory = super::import_graph::after(&metadata.inventory, &prepared)?;
            let mut import = PreparedAssetImport {
                request,
                token: prepared.token.clone(),
                entries,
                after_inventory,
            };
            super::import_projection::seal(root, bindings, metadata, &mut import, cancel)?;
            metadata.import = Some(import);
            prepared.discard().map_err(asset)?;
        }
    }
    Ok(())
}
