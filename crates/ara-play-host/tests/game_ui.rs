//! Resource packaging never depends on the player's working directory.
use ara_play_host::read_game_ui;

#[test]
fn resource_paths_and_nonfatal_failures_survive_portable_ui()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join("ui"))?;
    std::fs::create_dir(root.path().join("assets"))?;
    for (name, bytes) in [
        ("font.ttf", b"font".as_slice()),
        ("reticle.png", b"image"),
        ("click.ogg", b"click"),
    ] {
        std::fs::write(root.path().join("assets").join(name), bytes)?;
    }
    std::fs::write(root.path().join("ui/game.ui"), "<ui id='root'/>")?;
    std::fs::write(root.path().join("ui/game.css"), "")?;
    std::fs::write(root.path().join("ui/bindings.json"), "[]")?;
    std::fs::write(
        root.path().join("ui/resources.json"),
        r#"{
        "fonts":["assets/font.ttf"],"click":"assets/click.ogg",
        "images":{"reticle":"assets/reticle.png","missing":"assets/missing.png"},
        "music":[{"entity":"e_run","component":"SalvageRun","field":"alert","target":"e_music"}]
    }"#,
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
