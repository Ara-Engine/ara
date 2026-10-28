//! Pure logical/physical namespace bridge for editor importer adapters.
use ara_doc::{Document, Workspace};
fn within_authored_root(workspace: &Workspace, storage: &str) -> bool {
    let root = workspace.reference_root();
    root.is_empty()
        || storage
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/') && tail.len() > 1)
}

/// Editor import methods take physical storage paths. Only the configured
/// authored boundary is accepted; legacy workspaces keep their empty prefix.
pub(crate) fn source<'a>(workspace: &Workspace, storage: &'a str) -> Result<&'a str, String> {
    if storage.is_empty()
        || storage.contains([':', '\\'])
        || storage
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err("The source path must stay inside the project and use forward slashes".into());
    }
    if !within_authored_root(workspace, storage) {
        return Err("Choose a source inside authored content; project settings and generated files cannot be imported as assets".into());
    }
    Ok(workspace.reference_path(storage))
}
/// Compare logical sidecar paths while retaining the physical boundary, so a
/// project file with the same logical spelling cannot shadow an authored sidecar.
pub(crate) fn import_document<'a>(
    workspace: &'a Workspace,
    logical_source: &str,
) -> Option<&'a Document> {
    let sidecar = format!("{logical_source}.import.json");
    workspace.docs().find_map(|(storage, doc)| {
        (doc.kind == "import"
            && within_authored_root(workspace, storage)
            && workspace.reference_path(storage) == sidecar)
            .then_some(doc)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(id: &str) -> Document {
        Document::new(
            "import",
            ara_doc::AssetId::parse(id).expect("fixture asset ID"),
        )
    }
    #[test]
    fn layout3_uses_logical_sidecars_and_rejects_same_spelled_project_files() {
        let mut workspace = Workspace::default();
        workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        workspace.insert("models/rock.glb.import.json", document("as_project"));
        workspace.insert_with_asset_path(
            workspace.storage_path("models/rock.glb.import.json"),
            document("as_authored"),
            workspace.storage_path("models/rock.glb"),
        );
        let storage = workspace.storage_path("models/rock.glb");
        assert_eq!(
            source(&workspace, &storage).as_deref(),
            Ok("models/rock.glb")
        );
        assert_eq!(
            import_document(&workspace, "models/rock.glb").map(|doc| doc.id.as_str()),
            Some("as_authored")
        );
        assert!(source(&workspace, "models/rock.glb").is_err());
        assert!(
            source(
                &workspace,
                &format!("{}2/rock.glb", ara_doc::AUTHORING_ROOT)
            )
            .is_err()
        );
        assert!(
            source(
                &workspace,
                &format!("{}/../rock.glb", ara_doc::AUTHORING_ROOT)
            )
            .is_err()
        );
        assert!(source(&workspace, ara_doc::AUTHORING_ROOT).is_err());
    }
    #[test]
    fn legacy_source_and_sidecar_storage_remain_unchanged() {
        let mut workspace = Workspace::default();
        workspace.insert_with_asset_path(
            "models/rock.glb.import.json",
            document("as_legacy"),
            "models/rock.glb",
        );
        assert_eq!(
            source(&workspace, "models/rock.glb").as_deref(),
            Ok("models/rock.glb")
        );
        assert_eq!(
            import_document(&workspace, "models/rock.glb").map(|doc| doc.id.as_str()),
            Some("as_legacy")
        );
        assert!(source(&workspace, "../rock.glb").is_err());
    }

    #[test]
    fn source_media_bridge_matches_the_importers_authored_root() -> ara_ui::Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = crate::test_target()
            .join("editor-import-paths")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        for (name, media) in [
            ("current", "assets/rock.glb"),
            ("current-nested-game", "game/assets/rock.glb"),
        ] {
            let root = base.join(name);
            let mut workspace = Workspace::default();
            workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
            let storage = workspace.storage_path(media);
            let physical = root.join(&storage);
            std::fs::create_dir_all(
                physical
                    .parent()
                    .ok_or("Source fixture parent is missing")?,
            )?;
            let marker = serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION});
            std::fs::write(
                root.join("ara.project.json"),
                ara_json::to_canonical_string(&marker),
            )?;
            std::fs::write(&physical, b"real source bytes at the storage path")?;
            let logical = source(&workspace, &storage)?;
            assert_eq!(logical, media);
            // SourceAsset.path is relative to the root already selected by the
            // import pipeline. Preserve a genuine nested `game` folder.
            let input = ara_host::project::authored_root(&root).join(logical);
            assert_eq!(dunce::canonicalize(input)?, dunce::canonicalize(&physical)?);
        }
        Ok(())
    }
}
