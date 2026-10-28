//! Bounded UI fixtures preserve an existing project's reference root.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn panel_fixtures_follow_new_modern_and_existing_legacy_roots() -> ara_ui::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = crate::test_target()
        .join("panel-fixture-roots")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    for (name, version) in [("new", None), ("modern", Some(3)), ("legacy", Some(0))] {
        let root = base.join(name);
        std::fs::create_dir_all(&root)?;
        let marker = root.join("ara.project.json");
        let before = version.map(|version| {
            let mut value = json!({"name":"Existing fixture project","main":"levels/main.scene"});
            if version > 0 {
                value["layoutVersion"] = json!(version);
            }
            ara_json::to_canonical_string(&value)
        });
        if let Some(before) = &before {
            std::fs::write(&marker, before)?;
        }
        fixtures(&root)?;
        let content = ara_host::project::authored_root(&root);
        assert!(content.join("models/Avocado.glb").is_file());
        assert!(content.join("textures/checker.png").is_file());
        if let Some(before) = before {
            assert_eq!(
                std::fs::read_to_string(marker)?,
                before,
                "Fixture setup must not migrate the existing project marker"
            );
        }
        if version == Some(0) {
            assert!(
                !root.join(ara_doc::AUTHORING_ROOT).exists(),
                "Legacy fixture content was written into an invented authored root"
            );
        }
    }
    Ok(())
}
