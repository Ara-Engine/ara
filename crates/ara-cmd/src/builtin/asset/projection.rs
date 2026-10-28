use super::{
    AssetBlob, AssetInventory, AssetJsonFile, AssetMetadata, AssetOperationMetadata, Mode,
    access::Inventory, destination, files, invalid, references, selected,
};
use crate::EditCtx;
use ara_diag::Diagnostic;
use ara_doc::Workspace;
use ara_doc::file_state::FileState;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) fn validate_json(
    ctx: &EditCtx<'_, '_>,
    operation: &AssetOperationMetadata,
) -> Result<(), Diagnostic> {
    for file in &operation.json_files {
        if ctx.file_state(&file.path) != Some(&file.state) {
            return Err(invalid(format!(
                "JSON owner `{}` changed during preparation; retry",
                file.path
            )));
        }
    }
    Ok(())
}

pub(super) fn json(
    ctx: &mut EditCtx<'_, '_>,
    metadata: &AssetMetadata,
    operation: &AssetOperationMetadata,
    maps: &references::Mappings,
    source: &str,
    target: Option<&str>,
    mode: Mode,
) -> Result<(Vec<AssetBlob>, Vec<AssetJsonFile>), Diagnostic> {
    let native: BTreeSet<_> = ctx
        .read()
        .workspace()
        .docs()
        .map(|(path, _)| path.to_owned())
        .collect();
    let mut blobs = Vec::new();
    let mut next = Vec::new();
    for original in &operation.json_files {
        let is_selected = selected(&original.path, source);
        if mode == Mode::Delete && is_selected {
            continue;
        }
        if mode == Mode::Duplicate {
            next.push(original.clone());
            if !is_selected {
                continue;
            }
        }
        let mut file = original.clone();
        if is_selected && let Some(target) = target {
            file.path = destination(&file.path, source, target);
        }
        let before = file.value.clone();
        maps.rewrite_json(file.owner, file.context.as_ref(), &mut file.value);
        if mode == Mode::Duplicate
            && let Some(context) = file.context.as_ref()
        {
            file.context = maps
                .assets
                .get(context.as_str())
                .map(|(id, _)| id.clone())
                .or_else(|| Some(context.clone()));
        }
        if !native.contains(&file.path) && (is_selected || file.value != before) {
            let blob = files::json(ctx, metadata, &file.path, &file.value)?;
            file.state = blob.state.clone();
            blobs.push(blob);
        }
        next.push(file);
    }
    for file in &mut next {
        if let Some(state) = ctx.file_state(&file.path) {
            file.state = state.clone();
        }
    }
    Ok((blobs, next))
}

pub(super) fn graph(
    before: &AssetInventory,
    workspace: &Workspace,
    source: &str,
    target: Option<&str>,
    mode: Mode,
) -> Result<AssetInventory, Diagnostic> {
    let prefix = workspace.reference_root();
    let storage = |path: &str| {
        if prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{prefix}/{path}")
        }
    };
    let reference = |path: &str| workspace.reference_path(path).to_owned();
    let same = |a: &str, b: &str| {
        ara_doc::file_state::path_key_with_case(a, before.case_insensitive)
            == ara_doc::file_state::path_key_with_case(b, before.case_insensitive)
    };
    let states: BTreeMap<_, _> = workspace
        .file_entries()
        .map(|(path, state)| (path.to_owned(), state.clone()))
        .collect();
    let state = |path: &str| -> Result<(String, FileState), Diagnostic> {
        states
            .iter()
            .find(|(stored, _)| same(stored, &storage(path)))
            .map(|(path, state)| (reference(path), state.clone()))
            .ok_or_else(|| invalid(format!("projected source/input `{path}` is missing")))
    };
    let mut pending = VecDeque::new();
    for path in before.consumers.keys() {
        let is_selected = selected(&storage(path), source);
        if !is_selected || mode == Mode::Duplicate {
            pending.push_back((path.clone(), path.clone()));
        }
        if is_selected && let Some(target) = target {
            let mapped = destination(&storage(path), source, target);
            pending.push_back((path.clone(), reference(&mapped)));
        }
    }
    let mut consumers = BTreeMap::new();
    let mut visited = BTreeSet::new();
    while let Some((original, path)) = pending.pop_front() {
        if !visited.insert((original.clone(), path.clone())) {
            continue;
        }
        let mut consumer = before
            .consumers
            .get(&original)
            .ok_or_else(|| invalid("recognized companion discovery is unavailable"))?
            .clone();
        let (canonical, current) = state(&path)?;
        if current != consumer.state {
            return Err(invalid(format!(
                "projected source bytes for `{path}` require fresh importer validation"
            )));
        }
        consumer.state = current;
        let parent = canonical.rsplit_once('/').map_or("", |(parent, _)| parent);
        for input in &mut consumer.inputs {
            let original_input = input.path.clone();
            let expected = super::resolve_asset_input(parent, &input.uri)?;
            let (path, current) = state(&expected)?;
            if current != input.state {
                return Err(invalid(format!(
                    "projected URI bytes for `{path}` require fresh importer validation"
                )));
            }
            input.path = path;
            input.state = current;
            if before.consumers.contains_key(&original_input) {
                pending.push_back((original_input, input.path.clone()));
            }
        }
        if let Some(previous) = consumers.insert(canonical.clone(), consumer.clone())
            && previous != consumer
        {
            return Err(invalid(format!(
                "projected companion `{canonical}` has incompatible discovery results"
            )));
        }
    }
    Ok(AssetInventory {
        complete: before.complete,
        case_insensitive: before.case_insensitive,
        source_paths: consumers.keys().cloned().collect(),
        consumers,
    })
}

pub(super) fn file_view(workspace: &Workspace, token: &str, graph: &AssetInventory) -> Inventory {
    let prefix = workspace.reference_root().to_owned();
    Inventory {
        token: token.to_owned(),
        authored_prefix: prefix.clone(),
        case_insensitive: graph.case_insensitive,
        files: workspace
            .file_entries()
            .filter(|(path, _)| {
                prefix.is_empty() || *path == prefix || super::inside(path, &prefix)
            })
            .map(|(path, state)| (path.to_owned(), state.clone()))
            .collect(),
        source_paths: graph.source_paths.clone(),
        consumers: graph.consumers.clone(),
        complete: graph.complete,
    }
}
