use super::*;

#[test]
fn missing_and_corrupt_images_preserve_the_hud_and_report_authored_paths() -> Result<()> {
    let source: GameUi = serde_json::from_value(serde_json::json!({
        "markup":"<ui id='r'><image id='broken' src='broken'/><image id='missing' src='missing'/><text id='health' text='100 HP'/></ui>",
        "stylesheet":"ui {width:100%;height:100%;} image {width:20px;height:20px;} text {font-size:18px;}",
        "bindings":[], "images":{"broken":[0,1,2]},
        "resource_paths":{"image:broken":"assets/corrupt.png","image:missing":"assets/missing.png","font:0":"assets/corrupt.ttf"},
        "fonts":[[0,1,2]]
    }))?;
    let mut surface = Surface::plain(&source, [320, 180])?;
    let pixels = surface.image()?;
    assert!(
        pixels.pixels().any(|pixel| pixel[3] > 0),
        "body text must still draw"
    );
    let messages: Vec<_> = surface
        .diagnostics()
        .map(|diagnostic| diagnostic.message)
        .collect();
    for path in [
        "assets/corrupt.png",
        "assets/missing.png",
        "assets/corrupt.ttf",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(path)),
            "{messages:?}"
        );
    }
    surface.image()?;
    assert_eq!(
        surface.diagnostics().count(),
        0,
        "missing images report once per surface"
    );
    Ok(())
}

#[test]
fn project_fonts_keep_bundled_body_fallbacks() -> Result<()> {
    let mut source: GameUi = serde_json::from_value(serde_json::json!({
        "markup":"<ui id=\"font-fallback\"><button id=\"host\" action=\"host\" text=\"Host a crew\"/></ui>",
        "stylesheet":"ui {width:100%;height:100%;} button {font-family:Noto Sans;font-size:18px;padding:12px;}",
        "bindings":[]
    }))?;
    source
        .fonts
        .push(include_bytes!("../../../ara-ui-text/fonts/NotoNaskhArabic.ttf").to_vec());
    let mut surface = Surface::plain(&source, [1280, 720])?;
    surface.screen(Screen::Game)?;
    assert!(
        surface
            .text
            .assets
            .iter()
            .any(|font| font.bytes == include_bytes!("../../../ara-ui-text/fonts/NotoSans.ttf")),
        "project fonts must not replace the engine's body-font collection"
    );
    assert_eq!(surface.click("host")?[0].name, "host");
    Ok(())
}

#[test]
fn outpost_native_menu_and_settings_controls_layout_activate_and_resize() -> Result<()> {
    let source: GameUi = serde_json::from_value(serde_json::json!({
        "markup":include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../../samples/outpost/ui/game.ui")),
        "stylesheet":include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../../samples/outpost/ui/game.css")),
        "bindings":serde_json::from_str::<Value>(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../../samples/outpost/ui/bindings.json")))?
    }))?;
    let mut surface = Surface::plain(&source, [1280, 720])?;
    surface.screen(Screen::Main)?;
    assert_eq!(surface.click("host")?[0].name, "host");
    assert_eq!(surface.click("join")?[0].name, "join");
    assert_eq!(surface.click("open-settings")?[0].name, "settings");
    surface.screen(Screen::Settings)?;
    surface.settings(&[("master", 0.5), ("sensitivity", 1.5)], true)?;
    assert_eq!(surface.value("master"), Some(serde_json::json!(0.5)));
    assert_eq!(surface.value("invert"), Some(Value::Bool(true)));
    assert_eq!(surface.click("quality")?[0].name, "quality");
    assert_eq!(surface.click("window-mode")?[0].name, "window-mode");
    assert_eq!(surface.click("rebind-Interact")?[0].name, "rebind-Interact");
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-player/outpost-native-ui");
    surface.save(&directory.join("settings.png"))?;
    surface.resize([1920, 1080])?;
    assert_eq!(surface.click("apply")?[0].name, "apply");
    surface.screen(Screen::Main)?;
    surface.save(&directory.join("main-1080.png"))?;
    Ok(())
}
