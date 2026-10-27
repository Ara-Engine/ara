//! UI policy can relocate authored bootstrap sources without changing portable bytes.
use ara_play_host::{package_scene_for_export, read_game_ui};
use std::{fs, path::Path};

#[test]
fn configured_feature_paths_package_ui_and_exclude_project_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("ara.project.json"),
        r#"{"layoutVersion":3,"main":"scenes/main.scene"}"#,
    )?;
    ara_host::project_layout::write_support_files(root.path(), "UI paths")?;
    let authored = root.path().join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(authored.join("feature/hud"))?;
    fs::create_dir_all(authored.join("feature/media"))?;
    fs::create_dir_all(authored.join("settings"))?;
    fs::write(
        authored.join("scenes/main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
    )?;
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
        root.path().join(ara_asset::PROJECT_IMPORT_SETTINGS_FILE),
        &policy,
    )?;
    fs::write(
        authored.join("settings/ara.imports"),
        policy.replace("ip_project", "ip_feature"),
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
            .any(|doc| doc.id == "ip_project" || doc.kind == "project")
    );
    assert!(
        scene
            .documents
            .iter()
            .any(|doc| doc.id == "ip_feature" && doc.path == "settings/ara.imports")
    );
    Ok(())
}

#[test]
fn configured_missing_ui_and_escaping_resource_paths_fail_before_packaging()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("ara.project.json"),
        r#"{"layoutVersion":3}"#,
    )?;
    ara_host::project_layout::write_support_files(root.path(), "UI paths")?;
    assert!(read_game_ui(root.path())?.is_none());
    fs::write(
        root.path().join("settings/project.json"),
        r#"{"ui":{"main":"missing.ui"}}"#,
    )?;
    assert!(read_game_ui(root.path()).is_err());
    let authored = root.path().join(ara_doc::AUTHORING_ROOT);
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
fn migration_materializes_movable_legacy_ui_paths() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
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
    let before = read_game_ui(root.path())?.ok_or("legacy UI")?;
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
        after.markup.ends_with(&before.markup),
        "migration only adds stable asset identity metadata"
    );
    assert_eq!(after.stylesheet, before.stylesheet);
    assert_eq!(after.bindings, before.bindings);
    Ok(())
}
