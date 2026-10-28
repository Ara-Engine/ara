//! Resource packaging never depends on the player's working directory.
use ara_play_host::read_game_ui;

fn sources(authored: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(authored.join("ui"))?;
    std::fs::create_dir_all(authored.join("assets"))?;
    for (name, bytes) in [
        ("font.ttf", b"font".as_slice()),
        ("reticle.png", b"image"),
        ("click.ogg", b"click"),
    ] {
        std::fs::write(authored.join("assets").join(name), bytes)?;
    }
    std::fs::write(authored.join("ui/game.ui"), "<ui id='root'/>")?;
    std::fs::write(authored.join("ui/game.css"), "")?;
    std::fs::write(authored.join("ui/bindings.json"), "[]")?;
    std::fs::write(
        authored.join("ui/resources.json"),
        r#"{
        "fonts":["assets/font.ttf"],"click":"assets/click.ogg",
        "images":{"reticle":"assets/reticle.png","missing":"assets/missing.png"},
        "music":[{"entity":"e_run","component":"SalvageRun","field":"alert","target":"e_music"}]
    }"#,
    )?;
    Ok(())
}

#[test]
fn resource_paths_and_nonfatal_failures_survive_portable_ui()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    sources(&root.path().join(ara_doc::AUTHORING_ROOT))?;
    std::fs::write(
        root.path().join("ara.project.json"),
        ara_json::to_canonical_string(
            &serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
        ),
    )?;
    let ui = read_game_ui(root.path())?.ok_or("game UI")?;
    let portable: ara_play::GameUi = serde_json::from_slice(&serde_json::to_vec(&ui)?)?;
    assert_eq!(portable, ui);
    assert_eq!(ui.images["reticle"], b"image");
    assert_eq!(ui.fonts, [b"font".to_vec()]);
    assert_eq!(ui.click, b"click");
    assert_eq!(ui.resource_paths["image:missing"], "assets/missing.png");
    assert_eq!(ui.resource_diagnostics.len(), 1);
    assert!(ui.resource_diagnostics[0].contains("assets/missing.png"));
    assert_eq!(ui.music[0].target, "e_music");
    assert_eq!(ui.music[0].field, "alert");
    Ok(())
}

#[test]
fn legacy_resource_contract_survives_explicit_migration_before_read()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    sources(root.path())?;
    std::fs::write(root.path().join("ara.project.json"), "{}")?;
    let diagnostic = ara_host::project_layout::require_current_layout(root.path())
        .expect_err("legacy sources require migration before any read");
    assert_eq!(diagnostic.code, ara_diag::codes::PROJECT_MIGRATION_REQUIRED);
    let image = std::fs::read(root.path().join("assets/reticle.png"))?;
    let font = std::fs::read(root.path().join("assets/font.ttf"))?;
    let click = std::fs::read(root.path().join("assets/click.ogg"))?;
    ara_host::project_layout::Migration::plan(root.path(), None)
        .map_err(|d| d.to_string())?
        .apply()
        .map_err(|d| d.to_string())?;
    let migrated = read_game_ui(root.path())?.ok_or("migrated game UI")?;
    assert_eq!(migrated.images["reticle"], image);
    assert_eq!(migrated.fonts, [font]);
    assert_eq!(migrated.click, click);
    assert_eq!(migrated.resource_paths["image:missing"], "missing.png");
    assert_eq!(migrated.resource_paths["image:reticle"], "reticle.png");
    assert_eq!(migrated.resource_diagnostics.len(), 1);
    assert!(migrated.resource_diagnostics[0].contains("missing.png"));
    assert_eq!(migrated.music[0].target, "e_music");
    assert_eq!(migrated.music[0].field, "alert");
    Ok(())
}
