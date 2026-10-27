//! Worker-only preparation; commands consume only the returned immutable facts.
use super::{
    SpaceCheck,
    bindings::Bindings,
    importing,
    inventory::{Inventory, asset, one},
    metadata,
};
use ara_asset::{Cancellation, ProcessWorker};
use ara_diag::{Diagnostic, codes};
use ara_doc::{
    Workspace,
    file_state::{FileState, PreparedInventory},
};
use ara_host::{file_state::FileHistory, params::ApplyParams};
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub(super) struct Frozen {
    pub root: PathBuf,
    pub workspace: Workspace,
    pub schema: SchemaDb,
    pub batch: ara_cmd::Batch,
    pub worker: ProcessWorker,
    pub check_space: Arc<SpaceCheck>,
}

pub(super) fn run(
    frozen: Frozen,
    params: ApplyParams,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(f32, &str),
) -> Result<PreparedInventory, Vec<Diagnostic>> {
    cancel.check().map_err(asset)?;
    for path in [
        ".ara",
        ".ara/file-history",
        ".ara/cache",
        ".ara/cache/objects",
        ".ara/cache/actions",
    ] {
        if std::fs::symlink_metadata(frozen.root.join(path)).is_ok() {
            super::links::reject_linked_path(&frozen.root, path)?;
        }
    }
    if !frozen.batch.calls.is_empty()
        && frozen
            .batch
            .calls
            .iter()
            .all(|call| call.command == "build_profile.create")
    {
        progress(0.1, "Preparing build profile");
        return super::profiles::prepare(&frozen.root, &frozen.batch, cancel);
    }
    progress(0.01, "Inspecting authored files");
    let root = frozen
        .root
        .canonicalize()
        .map_err(|error| one(Diagnostic::error(codes::IO, error.to_string())))?;
    let mut bindings = Bindings {
        schema: frozen.schema,
        workspace: frozen.workspace,
        versions: Default::default(),
        preset: Default::default(),
        asset_hashes: Default::default(),
    };
    bindings.preset = ["settings/ara.imports", "ara.imports"]
        .into_iter()
        .find_map(|path| {
            bindings
                .workspace
                .docs()
                .find_map(|(name, document)| (name == path).then_some(document))
                .and_then(|document| document.record("pi_settings"))
        })
        .map(|record| {
            serde_json::from_value::<ara_asset::ProjectImportSettings>(Value::Object(
                record.clone(),
            ))
        })
        .transpose()
        .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?
        .unwrap_or_default()
        .preset;
    let imports: Vec<_> = params
        .commands
        .iter()
        .filter(|call| call.cmd == "asset.import")
        .collect();
    let readonly = params.dry_run.unwrap_or(false)
        || (!params.commands.is_empty()
            && params.commands.iter().all(|call| {
                call.cmd == "asset.import"
                    && call
                        .args
                        .as_ref()
                        .is_some_and(|args| args["dryRun"] == true)
            }));
    let mut inventory = Inventory::read(&root, &bindings, readonly, cancel)?;
    bindings.capture_assets(&inventory)?;
    let has_sources = inventory.authored.iter().any(|file| {
        file.scope == ara_host::project::AuthoredScope::Authored
            && file.role == ara_host::project::AuthoredRole::Source
            && (ara_importers::commands::importer_for_source(&file.path).is_ok()
                || bindings.workspace.docs().any(|(path, _)| {
                    path == bindings
                        .workspace
                        .storage_path(&format!("{}.import.json", file.path))
                }))
    });
    if has_sources || !imports.is_empty() {
        progress(0.08, "Checking installed importers");
        bindings.versions = frozen.worker.versions().map_err(asset)?;
    }
    let graph = inventory.dependencies(&root, &bindings, &frozen.worker, cancel, progress)?;
    // Pure command preview uses a frozen temporary authority. Production edits
    // still happen only after this worker returns its checked immutable metadata.
    if imports.len() > 1 {
        return Err(one(Diagnostic::error(
            codes::INVALID_PARAMS,
            "one asset.import job accepts the complete multi-file selection; combine sources into a single request",
        )));
    }
    let token = token(&inventory, &bindings, &frozen.batch);
    let mut metadata = metadata::base(&inventory, &bindings, &graph, token)?;
    if let Some(call) = imports.first() {
        let request = serde_json::from_value(call.args.clone().unwrap_or_else(|| json!({})))
            .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
        importing::prepare(
            importing::Context {
                root: &root,
                bindings: &bindings,
                inventory: &inventory,
                worker: &frozen.worker,
                check_space: frozen.check_space.as_ref(),
                cancel,
                progress,
            },
            &mut metadata,
            request,
        )?;
    }
    inventory.prepared.metadata = serde_json::to_value(&metadata)
        .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
    progress(0.86, "Preparing reference and identity changes");
    let preview = ara_cmd::builtin::preview_assets(
        &bindings.workspace,
        &bindings.schema,
        &frozen.batch,
        &mut inventory.prepared,
    )?;
    let mut metadata: ara_cmd::builtin::AssetMetadata =
        serde_json::from_value(inventory.prepared.metadata.clone())
            .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
    let history = FileHistory::new(&root);
    for native in preview.documents {
        cancel.check().map_err(asset)?;
        let bytes = ara_host::project::write_document(&native.document, &bindings.schema)
            .map_err(one)?
            .into_bytes();
        let state = if readonly {
            FileState::bytes(&bytes)
        } else {
            history.put(&bytes).map_err(one)?
        };
        let operation = metadata
            .operations
            .get_mut(&native.operation_key)
            .ok_or_else(|| {
                one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "native preview has no matching typed operation",
                ))
            })?;
        if operation.document_checksums.get(&native.path) != Some(&native.pure_hash) {
            return Err(one(Diagnostic::error(
                codes::STALE_BASE,
                "native document preview does not match its checked pure checksum",
            )
            .file(native.path)));
        }
        operation.document_states.insert(native.path, state);
    }
    inventory.prepared.metadata = serde_json::to_value(metadata)
        .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
    // Reproject with the real host-codec hashes so later commands in this same
    // batch observe exact XML/marker bytes rather than provisional Ara JSON.
    let preview = ara_cmd::builtin::preview_assets(
        &bindings.workspace,
        &bindings.schema,
        &frozen.batch,
        &mut inventory.prepared,
    )?;
    let metadata: ara_cmd::builtin::AssetMetadata =
        serde_json::from_value(inventory.prepared.metadata.clone())
            .map_err(|error| one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string())))?;
    for native in preview.documents {
        cancel.check().map_err(asset)?;
        let text =
            ara_host::project::write_document(&native.document, &bindings.schema).map_err(one)?;
        let operation = metadata
            .operations
            .get(&native.operation_key)
            .ok_or_else(|| {
                one(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "second native preview has no matching operation",
                ))
            })?;
        if operation.document_states.get(&native.path) != Some(&FileState::bytes(text.as_bytes()))
            || operation.document_checksums.get(&native.path) != Some(&native.pure_hash)
        {
            return Err(one(Diagnostic::error(
                codes::STALE_BASE,
                "native preview changed after exact host-codec hashes were attached",
            )
            .file(native.path)));
        }
    }
    for blob in preview.blobs {
        cancel.check().map_err(asset)?;
        let actual = if readonly {
            FileState::bytes(&blob.bytes)
        } else {
            history.put(&blob.bytes).map_err(one)?
        };
        if actual != blob.state {
            return Err(one(Diagnostic::error(
                codes::STALE_BASE,
                "canonical JSON preview differs from its staged bytes",
            )
            .file(blob.path)));
        }
    }
    cancel.check().map_err(asset)?;
    progress(0.96, "Checking prepared files are unchanged");
    super::links::reject_linked_tree(&root, bindings.workspace.reference_root(), cancel)?;
    if !bindings.workspace.reference_root().is_empty() && root.join("settings").exists() {
        super::links::reject_linked_tree(&root, "settings", cancel)?;
    }
    graph
        .verify(&root.join(bindings.workspace.reference_root()), cancel)
        .map_err(asset)?;
    FileHistory::new(&root)
        .revalidate(&inventory.prepared)
        .map_err(one)?;
    cancel.check().map_err(asset)?;
    Ok(inventory.prepared)
}

fn canonical(value: &Value) -> Vec<u8> {
    ara_json::to_canonical_string(&ara_json::sort_keys(value)).into_bytes()
}

fn token(inventory: &Inventory, bindings: &Bindings, batch: &ara_cmd::Batch) -> String {
    let documents: Vec<_> = bindings.workspace.docs().map(|(path, document)| json!({"path":path,"value":ara_doc::to_value(document, &bindings.schema)})).collect();
    let facts = json!({"files":inventory.prepared.entries,"directories":inventory.prepared.directories,"documents":documents,"versions":bindings.versions,"authoredRoot":bindings.workspace.reference_root(),"batch":batch});
    FileState::bytes(&canonical(&facts)).hash
}
