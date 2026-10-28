use super::{EntryKind, Error, Result};
use ara_asset::Cancellation;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone)]
pub(super) struct Incoming {
    pub group: usize,
    pub source: PathBuf,
    pub relative: String,
    pub kind: EntryKind,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

pub(super) struct Authority {
    path: PathBuf,
    kind: EntryKind,
}
pub(super) struct Scan {
    pub entries: Vec<Incoming>,
    pub authorities: Vec<Authority>,
}

pub(super) fn scan(
    paths: &[PathBuf],
    cancel: &Cancellation,
    limit: usize,
    mut report: impl FnMut(usize),
) -> Result<Scan> {
    if paths.is_empty() {
        return Err(Error::Invalid(
            "choose at least one external file or folder".into(),
        ));
    }
    let mut selected = BTreeMap::new();
    for path in paths {
        cancel.check()?;
        if !path.is_absolute() {
            return Err(Error::Invalid(format!(
                "import source must be an absolute OS path: {}",
                path.display()
            )));
        }
        no_links(path)?;
        let canonical = path.canonicalize()?;
        let metadata = fs::symlink_metadata(&canonical)?;
        let kind = kind(&metadata, &canonical)?;
        selected.insert(canonical, kind);
    }
    // Selecting a folder and one of its children authorizes one copy, not two.
    let authorities: Vec<_> = selected
        .iter()
        .filter(|(path, _)| {
            !selected.iter().any(|(parent, kind)| {
                *path != parent && *kind == EntryKind::Directory && path.starts_with(parent)
            })
        })
        .map(|(path, kind)| Authority {
            path: path.clone(),
            kind: *kind,
        })
        .collect();
    let mut entries = Vec::new();
    for (group, authority) in authorities.iter().enumerate() {
        let name = authority
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                Error::Invalid("drop a named file/folder rather than a filesystem root".into())
            })?;
        if ignored(name) {
            return Err(Error::Invalid(format!(
                "derived/system source `{name}` is not importable"
            )));
        }
        let mut pending = vec![(authority.path.clone(), name.to_owned())];
        while let Some((path, relative)) = pending.pop() {
            cancel.check()?;
            no_links(&path)?;
            let metadata = fs::symlink_metadata(&path)?;
            let kind = kind(&metadata, &path)?;
            if kind == EntryKind::Directory {
                if fs::symlink_metadata(path.join("ara.project.json")).is_ok() {
                    return Err(Error::Invalid(format!(
                        "{} is an Ara project; open/register the project, or drop an authored subfolder instead",
                        path.display()
                    )));
                }
                let mut children = Vec::new();
                for child in fs::read_dir(&path)? {
                    cancel.check()?;
                    let child = child?;
                    let name = child
                        .file_name()
                        .into_string()
                        .map_err(|_| Error::Invalid("import names must be UTF-8".into()))?;
                    if name.eq_ignore_ascii_case("ara.project.json") {
                        return Err(Error::Invalid(format!(
                            "{} contains an Ara project marker; drop authored content instead",
                            path.display()
                        )));
                    }
                    if !ignored(&name) {
                        children.push((child.path(), format!("{relative}/{name}")));
                    }
                }
                children.sort_by(|a, b| b.1.cmp(&a.1));
                pending.extend(children);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("ara.project.json"))
            {
                return Err(Error::Invalid("an Ara project marker cannot be imported as an asset; open/register its project".into()));
            }
            entries.push(Incoming {
                group,
                source: path,
                relative,
                kind,
                bytes: if kind == EntryKind::File {
                    metadata.len()
                } else {
                    0
                },
                modified: metadata.modified().ok(),
            });
            if entries.len() > limit {
                return Err(Error::Invalid(format!(
                    "import exceeds the {limit} entry budget; import a smaller selection"
                )));
            }
            report(entries.len());
            cancel.check()?;
        }
    }
    entries.sort_by(|a, b| {
        a.relative
            .cmp(&b.relative)
            .then_with(|| a.source.cmp(&b.source))
    });
    if let Some(mut common) = authorities
        .first()
        .and_then(|authority| authority.path.parent())
        .map(Path::to_owned)
    {
        while !authorities
            .iter()
            .all(|authority| authority.path.starts_with(&common))
        {
            let Some(parent) = common.parent() else {
                break;
            };
            common = parent.to_owned();
        }
        if authorities
            .iter()
            .all(|authority| authority.path.starts_with(&common))
        {
            for entry in &mut entries {
                entry.relative = entry
                    .source
                    .strip_prefix(&common)
                    .map_err(|_| {
                        Error::Invalid("selected files have no common filesystem root".into())
                    })?
                    .to_str()
                    .ok_or_else(|| Error::Invalid("import names must be UTF-8".into()))?
                    .replace('\\', "/");
            }
            entries.sort_by(|a, b| {
                a.relative
                    .cmp(&b.relative)
                    .then_with(|| a.source.cmp(&b.source))
            });
        }
    }
    Ok(Scan {
        entries,
        authorities,
    })
}

pub(super) fn authorized_file(
    base: &Path,
    uri: &str,
    authorities: &[Authority],
) -> Result<PathBuf> {
    if uri.is_empty() || uri.contains([':', '\\', '\0']) || Path::new(uri).is_absolute() {
        return Err(Error::Invalid(format!(
            "companion `{uri}` is not a local relative URI"
        )));
    }
    let path = base.join(uri);
    no_links(&path).map_err(|error|Error::Invalid(format!("companion `{uri}` is missing or linked: {error}; include its containing folder in the drop")))?;
    let canonical = path.canonicalize()?;
    if !authorities.iter().any(|authority| {
        canonical == authority.path
            || authority.kind == EntryKind::Directory && canonical.starts_with(&authority.path)
    }) {
        return Err(Error::Invalid(format!(
            "companion `{uri}` escapes the explicitly selected files/folder roots; drop a common containing folder or explicitly include that companion"
        )));
    }
    if !fs::symlink_metadata(&canonical)?.is_file() {
        return Err(Error::Invalid(format!(
            "companion `{uri}` is not a regular file"
        )));
    }
    Ok(canonical)
}

/// Rejects a path with a link or reparse point in it or in any ancestor, except
/// [`system_alias`]es.
pub(super) fn no_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        if linked(&metadata) && !system_alias(ancestor, &metadata) {
            return Err(Error::Invalid(format!(
                "linked/reparse source paths are not supported: {}",
                ancestor.display()
            )));
        }
    }
    Ok(())
}

/// A root-owned link directly under `/`: macOS's `/var`, `/tmp` and `/etc` alias
/// `/private/...`, so every temp and cache folder sits below one. No unprivileged
/// user can plant such a link.
fn system_alias(path: &Path, metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        path.parent() == Some(Path::new("/")) && metadata.uid() == 0
    }
    #[cfg(not(unix))]
    {
        let _ = (path, metadata);
        false
    }
}

pub(super) fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn kind(metadata: &fs::Metadata, path: &Path) -> Result<EntryKind> {
    if linked(metadata) {
        return Err(Error::Invalid(format!(
            "linked/reparse source paths are not supported: {}",
            path.display()
        )));
    }
    if metadata.is_dir() {
        Ok(EntryKind::Directory)
    } else if metadata.is_file() {
        Ok(EntryKind::File)
    } else {
        Err(Error::Invalid(format!(
            "special filesystem entries are not importable: {}",
            path.display()
        )))
    }
}

fn ignored(name: &str) -> bool {
    [".git", ".ara", ".ds_store", "thumbs.db"].contains(&name.to_lowercase().as_str())
}

pub(super) fn relative(path: &str, allow_empty: bool) -> Result<()> {
    if allow_empty && path.is_empty() {
        return Ok(());
    }
    if path.is_empty()
        || path.contains([':', '\\', '\0'])
        || Path::new(path).is_absolute()
        || path.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.starts_with('.')
                || part.ends_with(['.', ' '])
                || part.contains(['<', '>', '"', '|', '?', '*'])
                || part.chars().any(char::is_control)
                || part.len() > 255
                || part.encode_utf16().count() > 255
                || reserved(part)
        })
        || Path::new(path)
            .components()
            .any(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
    {
        return Err(Error::Invalid(format!(
            "import destination must be a canonical authored-relative path: `{path}`"
        )));
    }
    Ok(())
}

fn reserved(part: &str) -> bool {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Temp folders sit below a system alias on macOS (`/var` -> `/private/var`); a
    /// link the user made inside the selection is still refused.
    #[test]
    fn system_aliases_pass_and_user_links_are_refused() {
        let scratch = tempfile::tempdir().expect("scratch");
        let file = scratch.path().join("source.png");
        fs::write(&file, b"png").expect("file");
        no_links(&file).expect("a temp file below the system aliases");
        let link = scratch.path().join("linked");
        std::os::unix::fs::symlink(scratch.path(), &link).expect("link");
        assert!(no_links(&link.join("source.png")).is_err());
    }
}
