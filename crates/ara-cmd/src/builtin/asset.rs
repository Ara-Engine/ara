//! Asset organization is a single transaction: file states, document identities and references.
mod access;
mod dependencies;
mod files;
mod folder;
mod identity;
mod import;
mod metadata;
mod preview;
mod projection;
mod references;
mod remap;

use crate::{Command, EditCtx, Hints, Registry};
use access::Inventory;
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::{FileKind, FileState};
use ara_doc::{Document, Edit};
use ara_schema::Schema;
pub use folder::{FolderCreate, FolderCreateParams};
pub use import::{
    AssetImport, AssetImportParams, ImportConflict, ImportConflictItem, ImportConflictKind,
    ImportDisposition, ImportEntry, ImportEntryKind, ImportJson, ImportPlannedItem,
    PreparedAssetImport, PreparedImportPreflight,
};
pub use metadata::{
    AssetBlob, AssetConsumer, AssetDocument, AssetInventory, AssetJsonFile, AssetLocalInput,
    AssetMetadata, AssetOperationMetadata, AssetPreview, asset_operation_key,
};
pub use preview::preview_assets;
pub use references::{JsonOwner, json_uses_path, rewrite_json_paths};
pub use remap::{ImportRemap, remap_import};
/// Resolve an importer-decoded local URI inside the authored root, without I/O.
pub fn resolve_asset_input(parent: &str, uri: &str) -> Result<String, Diagnostic> {
    dependencies::resolve(parent, uri)
}
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

/// Parameters of `asset.move` and `asset.duplicate`.
#[derive(Schema, Deserialize, Serialize)]
#[ara(params, module = "core")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetDestinationParams {
    /// Existing authored-root-relative file or folder; source media includes its sidecar.
    pub source: String,
    /// Complete authored-root-relative destination, including the filename.
    pub destination: String,
}
/// Parameters of `asset.rename`.
#[derive(Schema, Deserialize, Serialize)]
#[ara(params, module = "core")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetRenameParams {
    /// Existing authored-root-relative file or folder.
    pub source: String,
    /// New basename, including the file extension; never a path.
    pub name: String,
}
/// Parameters of `asset.delete`.
#[derive(Schema, Deserialize, Serialize)]
#[ara(params, module = "core")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetDeleteParams {
    /// Authored-root-relative path; document references and local source URIs block deletion.
    pub source: String,
}

/// Relocates a file or folder while preserving every asset identity.
pub struct AssetMove;
impl Command for AssetMove {
    type Params = AssetDestinationParams;
    type Output = Value;
    const NAME: &'static str = "asset.move";
    const SCOPE: &'static str = "edit:assets";
    const SUMMARY: &'static str =
        "Move an authored file or folder, fixing references in the same undoable transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":"props/crate.prefab","destination":"warehouse/crate.prefab"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        apply(
            ctx,
            &params.source,
            Some(&params.destination),
            Mode::Move,
            &operation_key(Self::NAME, &params)?,
            "destination",
        )
    }
}
/// Renames one basename, including Windows case-only renames.
pub struct AssetRename;
impl Command for AssetRename {
    type Params = AssetRenameParams;
    type Output = Value;
    const NAME: &'static str = "asset.rename";
    const SCOPE: &'static str = "edit:assets";
    const SUMMARY: &'static str =
        "Rename an authored file or folder and repair its references, with undo.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":"props/crate.prefab","name":"supply-crate.prefab"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        if params.name.is_empty()
            || params.name.contains(['/', '\\', ':'])
            || matches!(params.name.as_str(), "." | "..")
        {
            return Err(invalid("name must be one filename, including its extension").path("name"));
        }
        let destination = Path::new(&params.source)
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(&params.name)
            .to_string_lossy()
            .replace('\\', "/");
        apply(
            ctx,
            &params.source,
            Some(&destination),
            Mode::Move,
            &operation_key(Self::NAME, &params)?,
            "name",
        )
    }
}
/// Copies an authored subtree, assigning new asset and addressable record identities.
pub struct AssetDuplicate;
impl Command for AssetDuplicate {
    type Params = AssetDestinationParams;
    type Output = Value;
    const NAME: &'static str = "asset.duplicate";
    const SCOPE: &'static str = "edit:assets";
    const SUMMARY: &'static str =
        "Duplicate authored assets with new IDs and remapped internal references, with undo.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":"props/crate.prefab","destination":"props/crate-copy.prefab"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        apply(
            ctx,
            &params.source,
            Some(&params.destination),
            Mode::Duplicate,
            &operation_key(Self::NAME, &params)?,
            "destination",
        )
    }
}
/// Removes an unreferenced authored subtree while retaining immutable undo snapshots.
pub struct AssetDelete;
impl Command for AssetDelete {
    type Params = AssetDeleteParams;
    type Output = Value;
    const NAME: &'static str = "asset.delete";
    const SCOPE: &'static str = "edit:assets";
    const SUMMARY: &'static str = "Safely delete an authored file or folder; report every inbound reference before changing anything.";
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: true,
    };
    fn examples() -> Vec<Value> {
        vec![json!({"source":"unused/crate.prefab"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        apply(
            ctx,
            &params.source,
            None,
            Mode::Delete,
            &operation_key(Self::NAME, &params)?,
            "source",
        )
    }
}

pub(super) fn register(registry: &mut Registry, db: &mut ara_schema::SchemaDb) {
    registry
        .command::<AssetMove>(db)
        .command::<AssetRename>(db)
        .command::<AssetDuplicate>(db)
        .command::<AssetDelete>(db);
    registry.command::<FolderCreate>(db);
    registry.job::<AssetImport>(db);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Move,
    Duplicate,
    Delete,
}
fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}
fn editable(path: &str) -> Result<(), Diagnostic> {
    let parts: Vec<_> = path.split('/').collect();
    if path.is_empty()
        || path.contains(['\\', ':'])
        || Path::new(path).is_absolute()
        || parts.iter().any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with(['.', ' '])
                || part.contains(['<', '>', '"', '|', '?', '*'])
                || part.chars().any(char::is_control)
                || part.len() > 255
                || part.encode_utf16().count() > 255
                || reserved_name(part)
        })
        || ["ara.project.json", "AGENTS.md", "thumbnail.png"].contains(&path)
    {
        return Err(invalid(
            "choose an authored file or folder inside the project; metadata, policy and derived roots stay in place",
        ));
    }
    Ok(())
}
fn reserved_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}
fn inside(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn selected(path: &str, source: &str) -> bool {
    inside(path, source) || path == format!("{source}.import.json")
}
fn destination(path: &str, source: &str, target: &str) -> String {
    format!("{target}{}", &path[source.len()..])
}

fn operation_key(command: &str, params: &impl Serialize) -> Result<String, Diagnostic> {
    asset_operation_key(
        command,
        &serde_json::to_value(params).map_err(|error| invalid(error.to_string()))?,
    )
}

#[derive(Clone)]
struct SourceDoc {
    path: String,
    reference_path: String,
    document: Document,
}

fn apply(
    ctx: &mut EditCtx<'_, '_>,
    source_ref: &str,
    target_ref: Option<&str>,
    mode: Mode,
    operation_key: &str,
    target_parameter: &str,
) -> Result<Value, Diagnostic> {
    editable(source_ref).map_err(|diagnostic| diagnostic.path("source"))?;
    if let Some(target) = target_ref {
        editable(target).map_err(|diagnostic| diagnostic.path(target_parameter))?;
    }
    let metadata = AssetMetadata::read(ctx.prepared_metadata())?;
    let operation_slot = metadata.slot(ctx, operation_key);
    let operation = metadata.operation(&operation_slot)?;
    let inventory = Inventory::checked(ctx, &metadata, &operation)?;
    let before_documents: std::collections::BTreeMap<_, _> = ctx
        .read()
        .workspace()
        .docs()
        .map(|(path, document)| (document.id.clone(), (path.to_owned(), document.clone())))
        .collect();
    let source_storage = inventory.storage(source_ref);
    let target_storage = target_ref.map(|target| inventory.storage(target));
    let source = source_storage.as_str();
    let target = target_storage.as_deref();
    inventory
        .editable(source)
        .map_err(|diagnostic| diagnostic.path("source"))?;
    if source.ends_with(".import.json") {
        return Err(
            invalid("choose the source media; its import sidecar follows automatically")
                .path("source"),
        );
    }
    if let Some(target) = target {
        inventory
            .editable(target)
            .map_err(|diagnostic| diagnostic.path(target_parameter))?;
        if target.ends_with(".import.json") {
            return Err(
                invalid("a source destination cannot be an import sidecar").path(target_parameter)
            );
        }
        if source == target
            || inside(&inventory.path_key(target), &inventory.path_key(source))
                && !inventory.same_path(source, target)
        {
            return Err(invalid(
                "destination must differ from the source and stay outside its subtree",
            )
            .path(target_parameter));
        }
    }
    let selected_files: Vec<_> = inventory
        .files
        .iter()
        .filter(|(path, _)| selected(path, source))
        .map(|(path, state)| (path.clone(), state.clone()))
        .collect();
    let documents: Vec<_> = ctx
        .read()
        .workspace()
        .docs()
        .filter(|(path, _)| selected(path, source))
        .map(|(path, document)| {
            Ok(SourceDoc {
                path: path.to_owned(),
                reference_path: inventory.reference(path)?.to_owned(),
                document: document.clone(),
            })
        })
        .collect::<Result<_, Diagnostic>>()?;
    if selected_files.is_empty() && documents.is_empty() {
        return Err(
            invalid(format!("`{source}` is not an authored file or folder")).path("source"),
        );
    }
    if documents
        .iter()
        .any(|source| source.document.kind == "project")
    {
        return Err(invalid(
            "project metadata and policy cannot be reorganized as assets",
        ));
    }
    if documents
        .iter()
        .any(|source| source.document.id == ctx.read().doc().id)
    {
        return Err(invalid(
            "run asset commands from ara.project.json so the batch's anchor stays loaded",
        ));
    }
    if documents
        .iter()
        .any(|source| !source.document.conflicts.is_empty())
    {
        return Err(invalid(
            "resolve the selected documents' merge conflicts before reorganizing them",
        ));
    }
    if let Some(target) = target {
        let occupied_documents: Vec<_> = ctx
            .read()
            .workspace()
            .docs()
            .map(|(path, _)| path)
            .collect();
        validate_destination(&inventory, &occupied_documents, source, target, mode)
            .map_err(|diagnostic| diagnostic.path(target_parameter))?;
        for document in &documents {
            let new = destination(&document.path, source, target);
            let extension = ctx
                .read()
                .schema()
                .kind(&document.document.kind)
                .ok_or_else(|| invalid("unregistered document kind"))?
                .extension
                .clone();
            if !new.ends_with(&format!(".{extension}")) {
                return Err(invalid(
                    "renaming must retain the document's file extension",
                ));
            }
        }
        if inventory
            .source_paths
            .contains(inventory.reference(source)?)
            && Path::new(source)
                .extension()
                .map(|value| value.to_string_lossy().to_lowercase())
                != Path::new(target)
                    .extension()
                    .map(|value| value.to_string_lossy().to_lowercase())
        {
            return Err(invalid(
                "renaming source media must retain its format extension",
            ));
        }
    }
    let selected_ids: BTreeSet<_> = documents
        .iter()
        .map(|doc| doc.document.id.to_string())
        .collect();
    projection::validate_json(ctx, &operation)?;
    let companions = dependencies::prepare(&inventory, source, target, mode)?;
    if mode == Mode::Delete {
        let mut blockers = references::blockers(
            ctx.read().workspace(),
            inventory.reference(source)?,
            &selected_ids,
        );
        blockers.extend(
            operation
                .json_files
                .iter()
                .filter(|file| {
                    !selected(&file.path, source)
                        && json_uses_path(file.owner, source_ref, &selected_ids, &file.value)
                })
                .map(|file| format!("{} (JSON resource/policy)", file.path)),
        );
        if !blockers.is_empty() {
            return Err(invalid(format!(
                "`{source}` is still used by:\n{}",
                blockers.join("\n")
            )));
        }
    }
    let mut maps = references::Mappings::new(
        inventory.reference(source)?,
        target
            .map(|target| inventory.reference(target))
            .transpose()?,
        &documents,
    );
    let mut created = Vec::new();
    if mode == Mode::Duplicate {
        let mut taken_assets: BTreeSet<_> = ctx
            .read()
            .workspace()
            .docs()
            .map(|(_, document)| document.id.to_string())
            .collect();
        let identity_token = format!("{}:{operation_slot}", metadata.token);
        let mut taken: BTreeSet<String> = ctx
            .read()
            .workspace()
            .docs()
            .flat_map(|(_, doc)| doc.records().keys().map(ToString::to_string))
            .collect();
        for document in &documents {
            let new_path = destination(
                &document.path,
                source,
                target.expect("duplicate has a destination"),
            );
            let prefix = ctx
                .read()
                .schema()
                .kind(&document.document.kind)
                .ok_or_else(|| invalid("unregistered native document kind"))?
                .id_prefix
                .clone();
            let new_id = identity::asset(
                &prefix,
                &identity_token,
                inventory.reference(&new_path)?,
                document.document.id.as_str(),
                &mut taken_assets,
            )?;
            maps.assets.insert(
                document.document.id.to_string(),
                (
                    new_id.clone(),
                    references::asset_path(
                        inventory.reference(&new_path)?,
                        &document.document.kind,
                    )
                    .to_owned(),
                ),
            );
            created.push((document.clone(), new_path.clone(), new_id));
            let addressable = ctx
                .read()
                .schema()
                .records_addressable(&document.document.kind);
            for record in document.document.records().keys() {
                let new = if record.prefix() == "e" || addressable {
                    identity::record(
                        record.prefix(),
                        &identity_token,
                        inventory.reference(&new_path)?,
                        &format!("{}:{record}", document.document.id),
                        &mut taken,
                    )?
                } else {
                    record.clone()
                };
                maps.records
                    .insert((document.document.id.to_string(), record.to_string()), new);
            }
        }
        for (source_doc, path, id) in &created {
            ctx.create_document(
                inventory.reference(path)?,
                &source_doc.document.kind,
                Some(id.as_str()),
            )?;
        }
    }
    let mut updated = 0usize;
    if mode == Mode::Move {
        for document in &documents {
            remove_document(ctx, document)?;
            let path = destination(
                &document.path,
                source,
                target.expect("move has a destination"),
            );
            ctx.apply_edit(Edit::Document {
                doc: document.document.id.clone(),
                path,
                kind: Some(document.document.kind.clone()),
            })?;
            for (id, record) in document.document.records() {
                ctx.apply_edit(Edit::Create {
                    doc: document.document.id.clone(),
                    id: id.clone(),
                    record: record.clone(),
                })?;
            }
        }
        let edits = references::fixups(ctx.read().workspace(), &maps);
        updated = edits.len();
        for edit in edits {
            ctx.apply_edit(edit)?;
        }
    } else if mode == Mode::Duplicate {
        for (source_doc, _, id) in &created {
            for (record_id, record) in source_doc.document.records() {
                let mut record = record.clone();
                maps.rewrite_record(&source_doc.document, &mut record);
                let new = maps
                    .records
                    .get(&(source_doc.document.id.to_string(), record_id.to_string()))
                    .expect("every copied record was mapped");
                ctx.apply_edit(Edit::Create {
                    doc: id.clone(),
                    id: new.clone(),
                    record,
                })?;
            }
        }
    } else {
        for document in &documents {
            remove_document(ctx, document)?;
        }
    }
    let document_paths: BTreeSet<_> = documents
        .iter()
        .map(|document| document.path.as_str())
        .collect();
    let companion_paths: Vec<_> = companions
        .iter()
        .map(|(path, _)| inventory.reference(path).map(str::to_owned))
        .collect::<Result<_, Diagnostic>>()?;
    let mut special: BTreeSet<String> = documents
        .iter()
        .map(|document| {
            target.map_or_else(
                || document.path.clone(),
                |target| destination(&document.path, source, target),
            )
        })
        .collect();
    special.extend(
        operation
            .json_files
            .iter()
            .filter(|file| selected(&file.path, source))
            .map(|file| {
                target.map_or_else(
                    || file.path.clone(),
                    |target| destination(&file.path, source, target),
                )
            }),
    );
    for (path, state) in file_plan(
        &selected_files,
        &document_paths,
        source,
        target,
        mode,
        companions,
    ) {
        if state.is_some() && special.contains(&path) {
            continue;
        }
        if state.is_some() {
            files::parents(ctx, &path)?;
        }
        ctx.set_file(&path, state)?;
    }
    let (blobs, mut next_json) =
        projection::json(ctx, &metadata, &operation, &maps, source, target, mode)?;
    files::sync_documents(
        ctx,
        &before_documents,
        &metadata,
        &operation,
        &operation_slot,
    )?;
    for file in &mut next_json {
        if let Some(state) = ctx.file_state(&file.path) {
            file.state = state.clone();
        }
    }
    let assets: Vec<_> = if mode == Mode::Duplicate {
        created
            .iter()
            .map(|(_, path, id)| Ok(json!({"id":id,"path":inventory.reference(path)?})))
            .collect::<Result<_, Diagnostic>>()?
    } else {
        documents.iter().map(|document|json!({"id":document.document.id,"path":target_ref.map_or_else(||document.reference_path.clone(),|target|destination(&document.reference_path,source_ref,target))})).collect()
    };
    let mut output = json!({"source":source_ref,"destination":target_ref,"assets":assets,"updatedReferences":updated,"companionPaths":companion_paths});
    if metadata.preview {
        output["_assetBlobs"] =
            serde_json::to_value(blobs).map_err(|error| invalid(error.to_string()))?;
        output["_assetInventory"] = serde_json::to_value(projection::graph(
            &operation.inventory,
            ctx.read().workspace(),
            source,
            target,
            mode,
        )?)
        .map_err(|error| invalid(error.to_string()))?;
        output["_assetJsonFiles"] =
            serde_json::to_value(next_json).map_err(|error| invalid(error.to_string()))?;
    }
    Ok(output)
}

fn file_plan(
    files: &[(String, FileState)],
    documents: &BTreeSet<&str>,
    source: &str,
    target: Option<&str>,
    mode: Mode,
    companions: Vec<(String, FileState)>,
) -> Vec<(String, Option<FileState>)> {
    let mut edits = Vec::new();
    // Old spellings go first, including case-only moves; the materializer applies
    // the final batch atomically from immutable snapshots, never direct renames.
    if mode != Mode::Duplicate {
        edits.extend(files.iter().rev().map(|(path, _)| (path.clone(), None)));
    }
    if let Some(target) = target {
        for (path, state) in files {
            // Ordinary persistence serializes duplicated docs with new IDs.
            if mode != Mode::Duplicate || !documents.contains(path.as_str()) {
                edits.push((destination(path, source, target), Some(state.clone())));
            }
        }
    }
    edits.extend(
        companions
            .into_iter()
            .map(|(path, state)| (path, Some(state))),
    );
    edits
}

fn validate_destination(
    inventory: &Inventory,
    documents: &[&str],
    source: &str,
    target: &str,
    mode: Mode,
) -> Result<(), Diagnostic> {
    let case_only = mode == Mode::Move && inventory.same_path(source, target);
    let target_key = inventory.path_key(target);
    for (path, state) in &inventory.files {
        if case_only && selected(path, source) {
            continue;
        }
        let key = inventory.path_key(path);
        if selected(&key, &target_key) || state.kind == FileKind::File && inside(&target_key, &key)
        {
            return Err(invalid(format!(
                "destination `{target}` already exists or collides with `{path}`; choose another name"
            )));
        }
    }
    for path in documents {
        if case_only && selected(path, source) {
            continue;
        }
        let key = inventory.path_key(path);
        if selected(&key, &target_key) || inside(&target_key, &key) {
            return Err(invalid(format!(
                "destination `{target}` collides with loaded document `{path}`; choose another name"
            )));
        }
    }
    Ok(())
}

fn remove_document(ctx: &mut EditCtx<'_, '_>, source: &SourceDoc) -> Result<(), Diagnostic> {
    let mut seen = BTreeSet::new();
    for record in source.document.records().keys() {
        for child in source.document.subtree(record) {
            if seen.insert(child.clone()) {
                ctx.apply_edit(Edit::Delete {
                    doc: source.document.id.clone(),
                    id: child,
                })?;
            }
        }
    }
    ctx.apply_edit(Edit::Document {
        doc: source.document.id.clone(),
        path: source.path.clone(),
        kind: None,
    })
}

#[cfg(test)]
mod tests;
