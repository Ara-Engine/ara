use super::external::{self, Incoming};
use super::{
    Conflict, ConflictKind, DestinationInventory, Disposition, Entry, EntryKind, Error,
    ImportConflict, Result,
};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) fn plan(
    incoming: &[Incoming],
    destination: &str,
    conflict: ImportConflict,
    inventory: &DestinationInventory,
) -> Result<Vec<Entry>> {
    Ok(plan_with_conflicts(incoming, destination, conflict, inventory)?.0)
}

pub(super) fn plan_with_conflicts(
    incoming: &[Incoming],
    destination: &str,
    conflict: ImportConflict,
    inventory: &DestinationInventory,
) -> Result<(Vec<Entry>, Vec<Conflict>)> {
    external::relative(destination, true)?;
    if inventory.token.is_empty() {
        return Err(Error::Invalid(
            "import requires a complete destination inventory token".into(),
        ));
    }
    let key =
        |path: &str| ara_doc::file_state::path_key_with_case(path, inventory.case_insensitive);
    let mut occupied = BTreeMap::new();
    for (path, entry) in &inventory.entries {
        external::relative(path, false)?;
        if entry.kind == EntryKind::File && entry.digest.is_none() {
            return Err(Error::Invalid(format!(
                "destination file `{path}` has no immutable snapshot digest; refresh the complete file inventory"
            )));
        }
        if occupied
            .insert(key(path), (path.clone(), entry.kind, false))
            .is_some()
        {
            return Err(Error::Invalid(format!(
                "ambiguous case spelling in destination inventory: {path}"
            )));
        }
    }
    let mut output = Vec::new();
    let mut conflicts = Vec::new();
    let mut directory_maps: BTreeMap<(usize, String), (String, bool, bool)> = BTreeMap::new();
    let mut source_destinations: BTreeMap<PathBuf, (String, bool)> = BTreeMap::new();
    for item in incoming {
        external::relative(&item.relative, false)?;
        let parent = directory_maps
            .iter()
            .filter(|((group, path), _)| {
                *group == item.group
                    && item
                        .relative
                        .strip_prefix(path)
                        .is_some_and(|tail| tail.starts_with('/'))
            })
            .max_by_key(|((_, path), _)| path.len());
        let (mut path, mut skipped, mut renamed) =
            if let Some(((_, prefix), (mapped, skipped, renamed))) = parent {
                (
                    format!("{mapped}{}", &item.relative[prefix.len()..]),
                    *skipped,
                    *renamed,
                )
            } else {
                (join(destination, &item.relative), false, false)
            };
        if let Some(media) = item
            .source
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".import.json"))
        {
            let media = item.source.with_file_name(media);
            let Some((mapped, media_skipped)) = source_destinations.get(&media) else {
                return Err(Error::Invalid(format!(
                    "drop source media together with its sidecar: {}",
                    item.source.display()
                )));
            };
            path = format!("{mapped}.import.json");
            skipped |= *media_skipped;
        }
        let collision = occupied.get(&key(&path)).cloned().or_else(|| {
            (item.kind == EntryKind::File && !path.ends_with(".import.json"))
                .then(|| occupied.get(&key(&format!("{path}.import.json"))).cloned())
                .flatten()
        });
        let mut disposition = if renamed {
            Disposition::Renamed
        } else {
            Disposition::Created
        };
        let mut previous = None;
        if skipped {
            disposition = Disposition::Skipped;
        } else if let Some((existing, kind, planned)) = collision {
            let existing_entry = inventory.entries.get(&existing);
            conflicts.push(Conflict {
                source: item.source.clone(),
                destination_logical_path: path.clone(),
                existing_asset_id: existing_entry.and_then(|entry| {
                    entry
                        .asset_id
                        .clone()
                        .or_else(|| entry.binding.as_ref().map(|binding| binding.id.clone()))
                }),
                kind: if key(&existing) != key(&path) {
                    ConflictKind::Sidecar
                } else if kind != item.kind {
                    ConflictKind::TypeMismatch
                } else if kind == EntryKind::Directory {
                    ConflictKind::Directory
                } else {
                    ConflictKind::File
                },
            });
            match conflict {
                ImportConflict::Skip => {
                    skipped = true;
                    disposition = Disposition::Skipped;
                    previous = inventory.entries.get(&existing).cloned();
                }
                ImportConflict::Rename => {
                    let requested = path.clone();
                    let maximum = occupied
                        .len()
                        .saturating_add(incoming.len())
                        .saturating_add(2);
                    let mut selected = None;
                    for number in 2..=maximum {
                        let candidate = suffixed(&requested, number, item.kind);
                        if !occupied.contains_key(&key(&candidate))
                            && !occupied.contains_key(&key(&format!("{candidate}.import.json")))
                        {
                            selected = Some(candidate);
                            break;
                        }
                    }
                    path = selected.ok_or_else(|| {
                        Error::Invalid("cannot allocate an unused import name".into())
                    })?;
                    renamed = true;
                    disposition = Disposition::Renamed;
                }
                ImportConflict::Replace => {
                    if kind != item.kind || key(&existing) != key(&path) {
                        return Err(Error::Invalid(format!(
                            "`{path}` collides with a different file/directory or orphan sidecar; choose Rename or Skip"
                        )));
                    }
                    if item.kind == EntryKind::Directory {
                        disposition = Disposition::Merged;
                        path = existing;
                    } else {
                        if planned {
                            return Err(Error::Invalid(format!(
                                "multiple incoming files target `{path}`; choose Rename or import them separately"
                            )));
                        }
                        path = existing;
                        disposition = Disposition::Replaced;
                        previous = inventory.entries.get(&path).cloned();
                    }
                }
            }
        }
        if !skipped {
            for (_, (existing, kind, _)) in occupied.range(..=key(&path)) {
                if *kind == EntryKind::File
                    && inside(&key(&path), &key(existing))
                    && key(&path) != key(existing)
                {
                    return Err(Error::Invalid(format!(
                        "import destination `{path}` is beneath existing file `{existing}`"
                    )));
                }
            }
            occupied.insert(key(&path), (path.clone(), item.kind, true));
        }
        if item.kind == EntryKind::Directory {
            directory_maps.insert(
                (item.group, item.relative.clone()),
                (path.clone(), skipped, renamed),
            );
        }
        source_destinations.insert(item.source.clone(), (path.clone(), skipped));
        output.push(Entry {
            source_reference: item.relative.clone(),
            source: Some(item.source.clone()),
            destination: path,
            kind: item.kind,
            disposition,
            bytes: item.bytes,
            digest: None,
            staged: None,
            asset_id: previous.and_then(|entry| {
                entry
                    .asset_id
                    .or_else(|| entry.binding.map(|binding| binding.id))
            }),
            native_document: false,
        });
    }
    // Explicit destination parents are additive; never replace existing files or
    // remove siblings while merging directories.
    let paths: Vec<_> = output
        .iter()
        .filter(|entry| entry.disposition != Disposition::Skipped)
        .map(|entry| entry.destination.clone())
        .collect();
    for path in paths {
        let mut parent = path.rsplit_once('/').map(|(parent, _)| parent.to_owned());
        while let Some(path) = parent {
            if let Some((_, kind, _)) = occupied.get(&key(&path)) {
                if *kind != EntryKind::Directory {
                    return Err(Error::Invalid(format!(
                        "destination parent `{path}` is a file"
                    )));
                }
            } else {
                occupied.insert(key(&path), (path.clone(), EntryKind::Directory, true));
                output.push(Entry {
                    source_reference: path.clone(),
                    source: None,
                    destination: path.clone(),
                    kind: EntryKind::Directory,
                    disposition: Disposition::Created,
                    bytes: 0,
                    digest: None,
                    staged: None,
                    asset_id: None,
                    native_document: false,
                });
            }
            parent = path.rsplit_once('/').map(|(parent, _)| parent.to_owned());
        }
    }
    output.sort_by(|a, b| {
        a.destination
            .cmp(&b.destination)
            .then_with(|| a.source.cmp(&b.source))
    });
    Ok((output, conflicts))
}

pub(super) fn join(parent: &str, path: &str) -> String {
    if parent.is_empty() {
        path.to_owned()
    } else {
        format!("{parent}/{path}")
    }
}
pub(super) fn inside(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(super) fn suffixed(path: &str, number: usize, kind: EntryKind) -> String {
    if kind == EntryKind::Directory {
        return format!("{path} ({number})");
    }
    let (parent, name) = path
        .rsplit_once('/')
        .map_or(("", path), |(parent, name)| (parent, name));
    let lower = name.to_lowercase();
    let compound = [
        ".import.json",
        ".export.json",
        ".types.json",
        ".module.toml",
        ".tar.gz",
        ".tar.xz",
        ".tar.bz2",
    ]
    .into_iter()
    .find(|extension| lower.ends_with(extension));
    let split = compound
        .map(|extension| name.len() - extension.len())
        .or_else(|| name.rfind('.').filter(|index| *index > 0));
    let renamed = if let Some(split) = split {
        format!("{} ({number}){}", &name[..split], &name[split..])
    } else {
        format!("{name} ({number})")
    };
    join(parent, &renamed)
}
