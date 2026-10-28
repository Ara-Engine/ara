//! Exact hash and owner metadata consumed by the pure asset commands.
use super::{
    bindings::Bindings,
    inventory::{Inventory, one},
};
use ara_cmd::builtin::{
    AssetConsumer, AssetInventory, AssetJsonFile, AssetLocalInput, AssetMetadata, JsonOwner,
};
use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::FileState;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn base(
    inventory: &Inventory,
    bindings: &Bindings,
    graph: &crate::DependencyInventory,
    token: String,
) -> Result<AssetMetadata, Vec<Diagnostic>> {
    let mut consumers = BTreeMap::new();
    let mut source_paths = BTreeSet::new();
    for source in graph.sources() {
        let source_state = state(inventory, &bindings.workspace.storage_path(&source.path))?;
        let mut inputs = Vec::new();
        for input in &source.inputs {
            inputs.push(AssetLocalInput {
                uri: input.uri.clone(),
                path: input.path.clone(),
                state: state(inventory, &bindings.workspace.storage_path(&input.path))?,
                blake3: input.digest.to_string(),
            });
        }
        source_paths.insert(source.path.clone());
        consumers.insert(
            source.path.clone(),
            AssetConsumer {
                state: source_state,
                blake3: source.digest.to_string(),
                inputs,
            },
        );
    }
    let mut json_files = Vec::new();
    let context = scene_context(bindings)?;
    for path in inventory.prepared.entries.keys() {
        let Some(owner) = json_owner(path, bindings)? else {
            continue;
        };
        if inventory.prepared.entries[path].is_none() {
            continue;
        }
        if inventory.prepared.entries[path]
            .as_ref()
            .is_some_and(|state| state.kind != ara_doc::file_state::FileKind::File)
        {
            if owner == JsonOwner::Generic {
                continue;
            }
            return Err(one(Diagnostic::error(
                codes::INVALID_REF,
                "configured JSON resources must name a file, not a directory",
            )
            .file(path)));
        }
        let bytes = inventory.content(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            one(Diagnostic::error(codes::INVALID_PARAMS, error.to_string()).file(path))
        })?;
        let value = ara_json::parse(text).map_err(|error| {
            one(Diagnostic::error(codes::JSON_SYNTAX, error.message)
                .file(path)
                .line_col(error.line, error.column))
        })?;
        if owner == JsonOwner::Generic && !has_explicit_reference(&value) {
            continue;
        }
        json_files.push(AssetJsonFile {
            path: path.clone(),
            owner,
            context: if owner == JsonOwner::Generic {
                None
            } else {
                context.clone()
            },
            value,
            state: state(inventory, path)?,
        });
    }
    let mut metadata = AssetMetadata::new(token);
    metadata.inventory = AssetInventory {
        complete: true,
        case_insensitive: cfg!(any(windows, target_os = "macos")),
        source_paths,
        consumers,
    };
    metadata.json_files = json_files;
    Ok(metadata)
}

pub(super) fn state(inventory: &Inventory, storage: &str) -> Result<FileState, Vec<Diagnostic>> {
    inventory
        .prepared
        .entries
        .get(storage)
        .and_then(Option::as_ref)
        .cloned()
        .ok_or_else(|| {
            one(Diagnostic::error(
                codes::STALE_BASE,
                "source/input is absent from the complete file inventory",
            )
            .file(storage))
        })
}

pub(super) fn json_owner(
    storage: &str,
    bindings: &Bindings,
) -> Result<Option<JsonOwner>, Vec<Diagnostic>> {
    if storage == ara_host::project::MARKER {
        return Ok(Some(JsonOwner::Marker));
    }
    if storage == ara_host::project_layout::PREFERENCES {
        return Ok(Some(JsonOwner::Policy));
    }
    if storage.starts_with("settings/builds/") && storage.ends_with(".export.json") {
        return Ok(Some(JsonOwner::ExportPreset));
    }
    let reference = bindings.workspace.reference_path(storage);
    let resources = super::ui_paths::configured(bindings, "resources", "ui/resources.json")?;
    let bindings_path = super::ui_paths::configured(bindings, "bindings", "ui/bindings.json")?;
    if ara_doc::file_state::same_path(reference, &resources) {
        Ok(Some(JsonOwner::UiResources))
    } else if ara_doc::file_state::same_path(reference, &bindings_path) {
        Ok(Some(JsonOwner::UiBindings))
    } else {
        Ok(generic_json(storage, bindings).then_some(JsonOwner::Generic))
    }
}

fn generic_json(storage: &str, bindings: &Bindings) -> bool {
    if !storage.ends_with(".json")
        || bindings
            .workspace
            .docs()
            .any(|(path, _)| ara_doc::file_state::same_path(path, storage))
    {
        return false;
    }
    let prefix = bindings.workspace.reference_root();
    if !prefix.is_empty() {
        return storage
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with('/'));
    }
    !storage.split('/').next().is_some_and(|root| {
        root.starts_with('.')
            || [
                "settings",
                "packages",
                "builds",
                "target",
                "node_modules",
                "spikes",
                "evidence",
                "tests",
            ]
            .contains(&root)
    })
}

pub(super) fn has_explicit_reference(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains('@'),
        serde_json::Value::Array(values) => values.iter().any(has_explicit_reference),
        serde_json::Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| key.contains('@') || has_explicit_reference(value)),
        _ => false,
    }
}

fn scene_context(bindings: &Bindings) -> Result<Option<ara_doc::AssetId>, Vec<Diagnostic>> {
    let Some(main) = ara_host::project_layout::policy(&bindings.workspace, "main")
        .filter(|value| !value.is_null())
    else {
        let defaults = if bindings.workspace.reference_root().is_empty() {
            ["levels/main.scene", ara_host::project_layout::MAIN_SCENE]
        } else {
            [ara_host::project_layout::MAIN_SCENE, "levels/main.scene"]
        };
        return Ok(bindings
            .workspace
            .find(defaults[0])
            .or_else(|| bindings.workspace.find(defaults[1]))
            .cloned());
    };
    let main = main.as_str().ok_or_else(|| {
        one(Diagnostic::error(
            codes::INVALID_REF,
            "main must be an authored scene path or asset reference",
        )
        .file(ara_host::project_layout::PREFERENCES)
        .path("main"))
    })?;
    let path = super::ui_paths::resolve(bindings, "main", main)?;
    let id=bindings.workspace.find(&path).filter(|id|bindings.workspace.doc(id.as_str()).is_some_and(|document|document.kind=="scene"))
        .ok_or_else(||one(Diagnostic::error(codes::UNRESOLVED_REF,format!("main reference `{main}` does not resolve to a loaded scene; set main to a scene path or valid @path#id")).file(ara_host::project_layout::PREFERENCES).path("main")))?;
    Ok(Some(id.clone()))
}

#[cfg(test)]
mod tests;
