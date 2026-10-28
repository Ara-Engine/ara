//! Importer proofs for the exact graph after the prepared import is committed.
use ara_cmd::builtin::{AssetConsumer, AssetInventory, AssetLocalInput};
use ara_diag::Diagnostic;
use ara_doc::file_state::FileState;

pub(super) fn after(
    initial: &AssetInventory,
    prepared: &crate::import::PreparedImport,
) -> Result<AssetInventory, Vec<Diagnostic>> {
    let mut inventory = initial.clone();
    let key = |path: &str| ara_doc::file_state::path_key_with_case(path, initial.case_insensitive);
    for source in &prepared.sources {
        let aliases: Vec<_> = inventory
            .consumers
            .keys()
            .filter(|path| key(path) == key(&source.path))
            .cloned()
            .collect();
        for alias in aliases {
            inventory.consumers.remove(&alias);
            inventory.source_paths.remove(&alias);
        }
        let parent = source
            .path
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        let mut inputs = Vec::new();
        for (uri, bytes) in &source.input.dependencies {
            let path = ara_cmd::builtin::resolve_asset_input(parent, uri)
                .map_err(|diagnostic| vec![diagnostic])?;
            inputs.push(AssetLocalInput {
                uri: uri.clone(),
                path,
                state: FileState::bytes(bytes),
                blake3: ara_asset::Digest::of(bytes).to_string(),
            });
        }
        inventory.source_paths.insert(source.path.clone());
        inventory.consumers.insert(
            source.path.clone(),
            AssetConsumer {
                state: FileState::bytes(&source.input.source),
                blake3: ara_asset::Digest::of(&source.input.source).to_string(),
                inputs,
            },
        );
    }
    Ok(inventory)
}

/// Refresh only immutable byte proofs. An unchanged consumer discovers the same
/// local URI set; a changed source itself requires a freshly validated binding.
pub(super) fn refresh(
    inventory: &mut AssetInventory,
    files: &[(String, FileState, String)],
    validated_sources: &std::collections::BTreeSet<String>,
) -> Result<(), Vec<Diagnostic>> {
    let key =
        |path: &str| ara_doc::file_state::path_key_with_case(path, inventory.case_insensitive);
    let files: std::collections::BTreeMap<_, _> = files
        .iter()
        .map(|(path, state, blake3)| (key(path), (path.clone(), state.clone(), blake3.clone())))
        .collect();
    for (path, consumer) in &mut inventory.consumers {
        let source_validated = validated_sources.contains(&key(path));
        if let Some((_, state, blake3)) = files.get(&key(path)) {
            if consumer.state != *state {
                return Err(vec![ara_diag::Diagnostic::error(
                    ara_diag::codes::INVALID_PARAMS,
                    format!(
                        "projected source `{path}` changed after importer validation; reprepare its final bytes"
                    ),
                )]);
            }
            consumer.blake3 = blake3.clone();
        }
        for input in &mut consumer.inputs {
            if let Some((path, state, blake3)) = files.get(&key(&input.path)) {
                if source_validated && input.state != *state {
                    return Err(vec![ara_diag::Diagnostic::error(
                        ara_diag::codes::INVALID_PARAMS,
                        "an imported consumer's local input changed during native/JSON remapping; import those final native inputs first, then validate the consumer",
                    )]);
                }
                input.path = path.clone();
                input.state = state.clone();
                input.blake3 = blake3.clone();
            }
        }
    }
    Ok(())
}
