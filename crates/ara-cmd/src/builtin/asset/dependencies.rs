//! Plans local-URI companion writes from a complete immutable source inventory.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path};

use ara_diag::Diagnostic;
use ara_doc::file_state::{FileKind, FileState};

use super::access::Inventory;
use super::{Mode, destination, inside, invalid, selected};

pub(super) fn prepare(
    inventory: &Inventory,
    source: &str,
    target: Option<&str>,
    mode: Mode,
) -> Result<Vec<(String, FileState)>, Diagnostic> {
    inventory.validate()?;
    let mut copies = BTreeMap::new();
    let mut blockers = BTreeSet::new();
    // Model discovery is independent of loaded sidecars. An unimported source is
    // just as capable of owning a buffer/texture as one with an import document.
    for (reference, consumer) in &inventory.consumers {
        let path = inventory.storage(reference);
        let is_selected = selected(&path, source);
        if !is_selected && mode != Mode::Duplicate {
            for input in &consumer.inputs {
                let input_path = inventory.storage(&input.path);
                if !selected(&input_path, source) {
                    continue;
                }
                // A Windows case-only rename preserves the URI's filesystem
                // meaning; every other relocation needs a source-format rewrite.
                if mode == Mode::Move
                    && target.is_some_and(|target| {
                        inventory.same_path(&input_path, &destination(&input_path, source, target))
                    })
                {
                    continue;
                }
                blockers.insert(format!("{path} (local URI {})", input.uri));
            }
        }
    }
    if !blockers.is_empty() {
        return Err(invalid(format!(
            "`{source}` is still used by local source URIs:\n{}\nMove the consumers together or update their source URIs before retrying.",
            blockers.into_iter().collect::<Vec<_>>().join("\n")
        )));
    }
    if mode == Mode::Delete {
        return Ok(Vec::new());
    }
    let target = target.ok_or_else(|| invalid("move and duplicate require a destination"))?;

    // Compare companion writes against the final projected tree, rather than a
    // stale original tree whose selected paths may themselves be relocating.
    let mut projected = inventory.files.clone();
    if mode == Mode::Move {
        projected.retain(|path, _| !selected(path, source));
    }
    for (path, state) in &inventory.files {
        if selected(path, source) {
            projected.insert(destination(path, source, target), state.clone());
        }
    }
    let mut pending: VecDeque<_> = inventory
        .consumers
        .keys()
        .filter(|reference| selected(&inventory.storage(reference), source))
        .map(|reference| {
            (
                reference.clone(),
                destination(&inventory.storage(reference), source, target),
            )
        })
        .collect();
    let mut visited = BTreeSet::new();
    // A companion can itself be a recognized source (for example an image or a
    // nested model). Its own immutable URI graph must relocate with its bytes.
    while let Some((reference, new_source)) = pending.pop_front() {
        if !visited.insert((reference.clone(), inventory.path_key(&new_source))) {
            continue;
        }
        let consumer = inventory
            .consumers
            .get(&reference)
            .ok_or_else(|| invalid("recognized companion discovery is unavailable"))?;
        let path = inventory.storage(&reference);
        let new_reference = inventory.reference(&new_source)?;
        let parent = new_reference
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        for input in &consumer.inputs {
            let old_input = inventory.storage(&input.path);
            let new_input = inventory.storage(&resolve(parent, &input.uri)?);
            inventory.editable(&new_input)?;
            let mapped = if selected(&old_input, source) {
                destination(&old_input, source, target)
            } else {
                old_input.clone()
            };
            if !inventory.same_path(&mapped, &new_input)
                && mode == Mode::Move
                && inside(&new_input, source)
            {
                return Err(invalid(format!(
                    "`{path}` has a URI pointing into its old folder (`{}`); update that source URI before moving",
                    input.uri
                )));
            }
            let state = inventory.files.get(&old_input).ok_or_else(|| {
                invalid(format!(
                    "snapshot input `{old_input}` is unavailable; refresh and retry"
                ))
            })?;
            let existing = projected
                .iter()
                .find(|(path, _)| inventory.same_path(path, &new_input))
                .map(|(_, state)| state);
            if existing.is_some_and(|existing| existing != state) {
                return Err(invalid(format!(
                    "relocating `{path}` would overwrite different companion data at `{new_input}`; choose another destination"
                )));
            }
            if copies
                .get(&new_input)
                .is_some_and(|existing| existing != state)
            {
                return Err(invalid(format!(
                    "selected sources require different companion data at `{new_input}`; move them separately"
                )));
            }
            // Do not implicitly convert an existing file into a directory.
            for (existing, state) in &projected {
                if state.kind == FileKind::File
                    && !inventory.same_path(existing, &new_input)
                    && inside(
                        &inventory.path_key(&new_input),
                        &inventory.path_key(existing),
                    )
                {
                    return Err(invalid(format!(
                        "companion destination `{new_input}` is beneath existing file `{existing}`"
                    )));
                }
            }
            if existing.is_none() {
                projected.insert(new_input.clone(), state.clone());
                copies.insert(new_input.clone(), state.clone());
            }
            if inventory.consumers.contains_key(&input.path) {
                pending.push_back((input.path.clone(), new_input));
            }
        }
    }
    Ok(copies.into_iter().collect())
}

/// Resolves the importer's already-decoded local path against an authored-relative
/// source parent. Never percent-decode this key a second time.
pub(super) fn resolve(parent: &str, uri: &str) -> Result<String, Diagnostic> {
    if uri.is_empty() || uri.contains([':', '\\']) || Path::new(uri).is_absolute() {
        return Err(invalid(format!(
            "local source URI `{uri}` must stay authored-relative"
        )));
    }
    let mut parts: Vec<String> = Path::new(parent)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    for component in Path::new(uri).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir if !parts.is_empty() => {
                parts.pop();
            }
            _ => {
                return Err(invalid(format!(
                    "local source URI `{uri}` escapes the authored root"
                )));
            }
        }
    }
    let path = parts.join("/");
    super::editable(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests;
