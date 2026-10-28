//! Missing-source checks follow references through the workspace's physical boundary.
use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn missing_model_checks_use_storage_paths_and_current_asset_identity() -> ara_ui::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = crate::test_target()
        .join("missing-model-paths")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    for modern in [false, true] {
        let root = base.join(if modern { "modern" } else { "legacy" });
        let mut index = ara_doc::AssetIndex::new();
        index.insert("as_modelpath", "models/rock.glb", Some("import".into()));
        let mut workspace = ara_doc::Workspace::new(index);
        if modern {
            workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        }
        let physical = root.join(workspace.storage_path("models/rock.glb"));
        std::fs::create_dir_all(physical.parent().ok_or("Source parent missing")?)?;
        std::fs::write(&physical, b"model fixture")?;
        let mut entities = Resolved::new();
        for (id, reference) in [
            ("e_modelone", "@models/old-name.glb#as_modelpath"),
            ("e_modeltwo", "@models/rock.glb#as_modelpath"),
            ("e_modelthree", "@models/old-name.glb#as_modelpath"),
            ("e_missingone", "@models/missing.glb"),
            ("e_missingtwo", "@models/missing.glb"),
        ] {
            entities.insert(
                id.into(),
                ara_doc::prefab::ResolvedEntity {
                    record: json!({"MeshRenderer":{"model":reference}})
                        .as_object()
                        .ok_or("Fixture record missing")?
                        .clone(),
                    parent: None,
                    order: None,
                    source: (
                        ara_doc::AssetId::parse("sc_pathcheck")
                            .map_err(|error| format!("Fixture scene ID: {error}"))?,
                        ara_doc::RecordId::parse(id)
                            .map_err(|error| format!("Fixture entity ID: {error}"))?,
                    ),
                },
            );
        }
        let view = DocumentView::with_imports(Arc::new(entities), Arc::new(Imports::new()));
        let checked = view.missing_sources(&root, &workspace, &BTreeSet::new());
        assert_eq!(
            checked,
            vec![
                ("models/missing.glb".into(), Some("e_missingone".into())),
                ("models/rock.glb".into(), None)
            ]
        );
        let known = checked.iter().map(|(path, _)| path.clone()).collect();
        std::fs::remove_file(physical)?;
        assert!(
            view.missing_sources(&root, &workspace, &known).is_empty(),
            "Already checked source paths must not do repeated disk work"
        );
    }
    Ok(())
}
