//! Pure adapters between logical authoring paths and cached browser storage keys.
use ara_host::project::{AuthoredFile, AuthoredRole, AuthoredScope};

pub(in crate::shell) fn existing_storage<'a>(
    files: &'a [AuthoredFile],
    logical: &str,
) -> Option<&'a str> {
    files
        .iter()
        .find(|file| matches!(&file.scope, AuthoredScope::Authored) && file.path == logical)
        .map(|file| file.storage_path.as_str())
}
/// Stable IDs repair stale reference paths before consulting the browser cache.
pub(in crate::shell) fn reference_storage<'a>(
    files: &'a [AuthoredFile],
    index: &ara_doc::AssetIndex,
    reference: &str,
) -> Option<&'a str> {
    let parts = ara_schema::grammar::parse_ref(reference).ok()?;
    let logical = parts.id.and_then(|id| index.path_of(id)).or(parts.path)?;
    existing_storage(files, logical)
}
/// New source files use the indexed parent; an empty browser folder denotes the
/// workspace's authored root, never project-level settings or generated outputs.
pub(in crate::shell) fn storage(
    files: &[AuthoredFile],
    workspace: &ara_doc::Workspace,
    logical: &str,
) -> Result<String, String> {
    if logical.contains(['\\', ':'])
        || (!logical.is_empty()
            && logical
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | "..")))
    {
        return Err("Use an authored-relative path with forward slashes and no traversal".into());
    }
    if let Some(storage) = existing_storage(files, logical) {
        return Ok(storage.into());
    }
    if logical.is_empty() {
        return Ok(workspace.reference_root().into());
    }
    let parent = files
        .iter()
        .filter(|file| {
            matches!(&file.scope, AuthoredScope::Authored)
                && matches!(&file.role, AuthoredRole::Folder)
        })
        .filter_map(|file| {
            let suffix = logical.strip_prefix(&file.path)?.strip_prefix('/')?;
            Some((file.path.len(), file.storage_path.as_str(), suffix))
        })
        .max_by_key(|(length, _, _)| *length);
    Ok(parent.map_or_else(
        || workspace.storage_path(logical),
        |(_, storage, suffix)| format!("{storage}/{suffix}"),
    ))
}
/// Cached files and live logical documents avoid obvious collisions; the typed
/// host command remains authoritative if another participant creates the name.
pub(in crate::shell) fn available(
    files: &[AuthoredFile],
    workspace: &ara_doc::Workspace,
    logical: &str,
) -> bool {
    !files
        .iter()
        .any(|file| matches!(&file.scope, AuthoredScope::Authored) && file.path == logical)
        && workspace.index().id_of(logical).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder(path: &str, storage: &str, scope: AuthoredScope) -> AuthoredFile {
        AuthoredFile {
            path: path.into(),
            storage_path: storage.into(),
            scope,
            role: AuthoredRole::Folder,
        }
    }
    #[test]
    fn storage_uses_authored_scope_and_the_deepest_cached_parent() {
        let files = vec![
            folder("settings", "settings", AuthoredScope::Project),
            folder("settings", "game/settings", AuthoredScope::Authored),
            folder(
                "settings/builds",
                "game/settings/builds",
                AuthoredScope::Authored,
            ),
        ];
        let mut workspace = ara_doc::Workspace::default();
        workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        assert_eq!(
            storage(&files, &workspace, "settings/builds/NewBehaviour.cs").as_deref(),
            Ok("game/settings/builds/NewBehaviour.cs")
        );
        assert_eq!(existing_storage(&files, "settings"), Some("game/settings"));
        assert_eq!(
            storage(&files, &workspace, ""),
            Ok(ara_doc::AUTHORING_ROOT.into())
        );
        assert_eq!(
            storage(&[], &workspace, "First.scene"),
            Ok(workspace.storage_path("First.scene"))
        );
        assert!(storage(&files, &workspace, "../Escaped.cs").is_err());
        assert!(storage(&files, &workspace, "/Escaped.cs").is_err());
        assert!(storage(&files, &workspace, "nested\\Escaped.cs").is_err());
        let legacy = ara_doc::Workspace::default();
        assert_eq!(
            storage(&[], &legacy, "First.scene"),
            Ok("First.scene".into())
        );
        assert_eq!(storage(&[], &legacy, ""), Ok(String::new()));
    }
    #[test]
    fn project_scope_files_do_not_block_same_named_authored_folders() {
        let mut files = vec![folder(
            "settings/builds",
            "settings/builds",
            AuthoredScope::Project,
        )];
        let workspace = ara_doc::Workspace::default();
        assert!(available(&files, &workspace, "settings/builds"));
        files.push(folder(
            "settings/builds",
            "game/settings/builds",
            AuthoredScope::Authored,
        ));
        assert!(!available(&files, &workspace, "settings/builds"));
    }
    #[test]
    fn references_reveal_current_storage_by_id_instead_of_a_stale_logical_path() {
        let files = vec![AuthoredFile {
            path: "Materials/Renamed.graph".into(),
            storage_path: "game/Materials/Renamed.graph".into(),
            scope: AuthoredScope::Authored,
            role: AuthoredRole::Document {
                name: "graph".into(),
            },
        }];
        let mut index = ara_doc::AssetIndex::new();
        index.insert("gr_stable", "Materials/Renamed.graph", Some("graph".into()));
        assert_eq!(
            reference_storage(&files, &index, "@Materials/Old.graph#gr_stable"),
            Some("game/Materials/Renamed.graph")
        );
        assert_eq!(
            reference_storage(&files, &index, "@#gr_stable"),
            Some("game/Materials/Renamed.graph")
        );
        assert_eq!(
            reference_storage(&files, &index, "@Materials/Renamed.graph"),
            Some("game/Materials/Renamed.graph")
        );
        assert!(reference_storage(&files, &index, "Materials/Renamed.graph").is_none());
        assert!(reference_storage(&files, &index, "@../Escaped.graph#gr_stable").is_none());
    }
}
