//! Export reads current authored roots; legacy projects migrate before packaging.
use ara_play_host::{package_scene_for_export, read_game_ui};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn scratch() -> Result<tempfile::TempDir, std::io::Error> {
    let root = std::env::var_os("ARA_TEST_ROOT")
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(windows) {
                PathBuf::from("D:/AraBuild/ux-hub-evidence")
            } else {
                std::env::temp_dir()
            }
        })
        .join("export-ui-read-tests");
    fs::create_dir_all(&root)?;
    tempfile::Builder::new()
        .prefix("ui-paths-")
        .tempdir_in(root)
}

fn modern_project(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"scenes/main.scene"}),
        ),
    )?;
    let authored = ara_host::project::authored_root(root);
    fs::create_dir_all(authored.join("scenes"))?;
    fs::create_dir_all(root.join("settings"))?;
    fs::write(
        authored.join("scenes/main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
    )?;
    Ok(authored)
}

#[test]
fn configured_feature_paths_package_ui_short_references_and_exclude_project_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let root = scratch()?;
    let authored = modern_project(root.path())?;
    for folder in ["feature/hud", "feature/media", "settings", "props"] {
        fs::create_dir_all(authored.join(folder))?;
    }
    fs::write(
        authored.join("feature/hud/panel.ui"),
        "<!-- ara:asset ui_feature -->\n<ui id='root'/>",
    )?;
    fs::write(authored.join("feature/hud/style.css"), ".reticle {}")?;
    fs::write(authored.join("feature/hud/fields.json"), "[]")?;
    fs::write(
        authored.join("feature/hud/media.json"),
        r#"{"fonts":["feature/media/font.ttf"],"images":{"reticle":"feature/media/reticle.png"}}"#,
    )?;
    fs::write(authored.join("feature/media/font.ttf"), b"font")?;
    fs::write(authored.join("feature/media/reticle.png"), b"reticle")?;
    fs::write(
        root.path().join("settings/project.json"),
        r#"{"ui":{"main":"@stale/main.ui#ui_feature","stylesheet":"feature/hud/style.css","bindings":"feature/hud/fields.json","resources":"feature/hud/media.json"}}"#,
    )?;
    let policy = ara_asset::ProjectImportSettings::default().canonical_text()?;
    fs::write(
        root.path()
            .join(ara_asset::LEGACY_PROJECT_IMPORT_SETTINGS_FILE),
        &policy,
    )?;
    fs::write(
        root.path().join(ara_asset::PROJECT_IMPORT_SETTINGS_FILE),
        policy.replace("ip_project", "ip_scoped"),
    )?;
    fs::write(
        authored.join("settings/ara.imports"),
        policy.replace("ip_project", "ip_feature"),
    )?;
    fs::write(
        authored.join("props/box.prefab"),
        r#"{"$schema":"ara://schema/prefab/1","id":"pf_box","records":{"e_box":{"name":"Box"}}}"#,
    )?;
    fs::write(
        authored.join("scenes/main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{"e_use":{"prefab":"@props/box.prefab#pf_box"}}}"#,
    )?;
    // Project settings and packages must never contribute schemas to the player.
    fs::create_dir_all(root.path().join("packages/fake"))?;
    fs::write(
        root.path().join("packages/fake/ignored.types.json"),
        "not a type document",
    )?;
    fs::write(
        root.path().join("settings/ignored.types.json"),
        "not a type document",
    )?;
    fs::write(
        authored.join("feature/authored.types.json"),
        r#"{"$schema":"ara://schema/types/1","types":{},"sentinel":"authored"}"#,
    )?;
    let ui = read_game_ui(root.path())?.ok_or("UI")?;
    assert_eq!(ui.fonts, [b"font".to_vec()]);
    assert_eq!(ui.images["reticle"], b"reticle");
    let scene =
        package_scene_for_export(root.path(), Path::new("unused-import-worker"), None, |_| {
            Ok(())
        })?;
    assert_eq!(scene.ui, Some(ui));
    assert!(
        !scene
            .documents
            .iter()
            .any(|doc| matches!(doc.id.as_str(), "ip_project" | "ip_scoped")
                || doc.kind == "project")
    );
    assert!(
        scene
            .documents
            .iter()
            .any(|doc| doc.id == "ip_feature" && doc.path == "settings/ara.imports")
    );
    assert!(
        scene
            .types
            .iter()
            .any(|schema| schema["sentinel"] == "authored")
    );
    let (workspace, main) = scene.workspace()?;
    assert_eq!(main.as_str(), "sc_main");
    assert_eq!(workspace.path_of("sc_main"), Some("scenes/main.scene"));
    assert_eq!(
        workspace
            .find("@props/box.prefab#pf_box")
            .map(|id| id.as_str()),
        Some("pf_box")
    );
    assert_eq!(
        workspace
            .doc("sc_main")
            .ok_or("main scene")?
            .record("e_use")
            .ok_or("prefab instance")?["prefab"],
        "@props/box.prefab#pf_box"
    );
    Ok(())
}

#[test]
fn configured_missing_ui_and_escaping_resource_paths_fail_before_packaging()
-> Result<(), Box<dyn std::error::Error>> {
    let root = scratch()?;
    let authored = modern_project(root.path())?;
    assert!(read_game_ui(root.path())?.is_none());
    fs::write(
        root.path().join("settings/project.json"),
        r#"{"ui":{"main":"missing.ui"}}"#,
    )?;
    assert!(read_game_ui(root.path()).is_err());
    fs::create_dir_all(authored.join("ui"))?;
    fs::write(authored.join("ui/game.ui"), "<ui id='root'/>")?;
    fs::write(authored.join("ui/game.css"), "")?;
    fs::write(authored.join("ui/bindings.json"), "[]")?;
    fs::write(
        authored.join("ui/resources.json"),
        r#"{"images":{"outside":"../outside.png"}}"#,
    )?;
    fs::write(root.path().join("outside.png"), b"outside")?;
    fs::write(root.path().join("settings/project.json"), "{}")?;
    assert!(read_game_ui(root.path()).is_err());
    fs::write(
        root.path().join("settings/project.json"),
        r#"{"ui":{"resources":"missing.json"}}"#,
    )?;
    assert!(read_game_ui(root.path()).is_err());
    #[cfg(unix)]
    {
        fs::write(root.path().join("settings/project.json"), "{}")?;
        std::os::unix::fs::symlink(root.path().join("outside.png"), authored.join("linked.png"))?;
        fs::write(
            authored.join("ui/resources.json"),
            r#"{"images":{"outside":"linked.png"}}"#,
        )?;
        assert!(
            read_game_ui(root.path()).is_err(),
            "canonical symlink escapes cannot be packed"
        );
    }
    Ok(())
}

#[test]
fn legacy_ui_sources_require_migration_without_policy_or_scaffold_creation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = scratch()?;
    fs::write(root.path().join("ara.project.json"), "{}")?;
    fs::create_dir_all(root.path().join("ui"))?;
    let markup = "<ui id='hud'><text id='title' text='Legacy'/></ui>\n";
    for (name, text) in [
        ("game.ui", markup),
        ("game.css", ".title {}\n"),
        ("bindings.json", "[]"),
    ] {
        fs::write(root.path().join("ui").join(name), text)?;
    }
    let diagnostic = ara_host::project_layout::require_current_layout(root.path())
        .expect_err("legacy sources require migration before any read");
    assert_eq!(diagnostic.code, ara_diag::codes::PROJECT_MIGRATION_REQUIRED);
    assert_eq!(fs::read_to_string(root.path().join("ui/game.ui"))?, markup);
    assert_eq!(
        fs::read_to_string(root.path().join("ui/game.css"))?,
        ".title {}\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("ui/bindings.json"))?,
        "[]"
    );
    assert!(!root.path().join("settings").exists());
    assert!(!root.path().join(".ara").exists());
    assert!(!root.path().join(ara_doc::AUTHORING_ROOT).exists());
    Ok(())
}

#[test]
fn migration_materializes_movable_legacy_ui_paths() -> Result<(), Box<dyn std::error::Error>> {
    let root = scratch()?;
    fs::write(root.path().join("ara.project.json"), "{}")?;
    fs::create_dir_all(root.path().join("ui"))?;
    for (file, text) in [
        ("game.ui", "<ui id='root'/>"),
        ("game.css", ""),
        ("bindings.json", "[]"),
        ("resources.json", "{}"),
    ] {
        fs::write(root.path().join("ui").join(file), text)?;
    }
    let diagnostic = ara_host::project_layout::require_current_layout(root.path())
        .expect_err("legacy sources require migration before any read");
    assert_eq!(diagnostic.code, ara_diag::codes::PROJECT_MIGRATION_REQUIRED);
    let before = fs::read_to_string(root.path().join("ui/game.ui"))?;
    let stylesheet = fs::read_to_string(root.path().join("ui/game.css"))?;
    let bindings: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("ui/bindings.json"))?)?;
    ara_host::project_layout::Migration::plan(root.path(), None)
        .map_err(|error| error.to_string())?
        .apply()
        .map_err(|error| error.to_string())?;
    let policy =
        ara_host::project_layout::preferences(root.path()).map_err(|error| error.to_string())?;
    assert_eq!(
        policy["ui"],
        serde_json::json!({"main":"ui/game.ui","stylesheet":"ui/game.css","bindings":"ui/bindings.json","resources":"ui/resources.json"})
    );
    let after = read_game_ui(root.path())?.ok_or("migrated UI")?;
    assert!(
        after.markup.ends_with(&before),
        "migration only adds stable asset identity metadata"
    );
    assert_eq!(after.stylesheet, stylesheet);
    assert_eq!(serde_json::to_value(after.bindings)?, bindings);
    Ok(())
}
