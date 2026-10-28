//! Bounded UI fixtures require current projects and never migrate legacy inputs.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn tree(
    root: &std::path::Path,
) -> ara_ui::Result<std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>> {
    fn visit(
        root: &std::path::Path,
        at: &std::path::Path,
        out: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) -> ara_ui::Result<()> {
        for entry in std::fs::read_dir(at)? {
            let entry = entry?;
            let path = entry.path();
            let key = path.strip_prefix(root)?.to_path_buf();
            if entry.file_type()?.is_dir() {
                out.insert(key, None);
                visit(root, &path, out)?;
            } else {
                out.insert(key, Some(std::fs::read(path)?));
            }
        }
        Ok(())
    }
    let mut out = std::collections::BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

#[test]
fn panel_fixtures_follow_current_roots_and_preserve_existing_markers() -> ara_ui::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = crate::test_target()
        .join("panel-current-fixtures")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    for existing in [false, true] {
        let root = base.join(if existing { "existing" } else { "new" });
        std::fs::create_dir_all(&root)?;
        let marker = json!({"name":"Existing fixture project","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION});
        let before = ara_json::to_canonical_string(&marker);
        if existing {
            std::fs::write(root.join("ara.project.json"), &before)?;
        }
        fixtures(&root)?;
        let content = root.join(ara_doc::AUTHORING_ROOT);
        assert!(content.join("models/Avocado.glb").is_file());
        assert!(content.join("textures/checker.png").is_file());
        if existing {
            assert_eq!(
                std::fs::read_to_string(root.join("ara.project.json"))?,
                before,
                "Fixture setup must preserve the existing current marker"
            );
        } else {
            let marker: Value =
                ara_json::parse(&std::fs::read_to_string(root.join("ara.project.json"))?)?;
            assert_eq!(marker["layoutVersion"], ara_doc::PROJECT_LAYOUT_VERSION);
            assert!(content.join("levels/main.scene").is_file());
        }
    }
    Ok(())
}

#[test]
fn panel_fixtures_reject_legacy_projects_without_writing() -> ara_ui::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = crate::test_target()
        .join("panel-legacy-fixture-rejection")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    for version in [0, 3] {
        let root = base.join(version.to_string());
        std::fs::create_dir_all(root.join("levels"))?;
        let marker = if version == 0 {
            json!({"name":"Legacy fixture input","main":"levels/main.scene"})
        } else {
            json!({"name":"Preview3 fixture input","main":"levels/main.scene","layoutVersion":3})
        };
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(&marker),
        )?;
        std::fs::write(
            root.join("levels/main.scene"),
            b"original legacy scene bytes",
        )?;
        let before = tree(&root)?;
        let error =
            fixtures(&root).expect_err("Legacy fixture setup must require explicit migration");
        assert!(error.to_string().contains("migrate"), "{error}");
        assert_eq!(
            tree(&root)?,
            before,
            "Rejected fixture setup must preserve every file and directory, including marker bytes"
        );
        assert!(!root.join(ara_doc::AUTHORING_ROOT).exists());
    }
    Ok(())
}
