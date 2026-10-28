use super::*;
use ara_platform::Event;

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

fn fixture() -> Result<Surface> {
    let source: GameUi = serde_json::from_value(serde_json::json!({
        "markup":"<ui id='root'><panel id='main-menu'><field id='address' text='' /><button id='join' text='Join' action='join' /></panel><panel id='pause'><button id='resume' text='Resume' action='resume' /></panel><panel id='hud'><button id='hud-action' text='HUD' action='game' /></panel></ui>",
        "stylesheet":"ui {width:100%;height:100%;} field {width:180px;height:32px;padding:4px;} button {width:180px;height:32px;padding:4px;}",
        "bindings":[]
    }))?;
    Surface::plain(&source, [400, 300], 1.)
}

#[test]
fn platform_unicode_ime_and_primary_clipboard_reach_the_actual_game_field_once() -> Result<()> {
    let mut surface = fixture()?;
    surface.screen(Screen::Main)?;
    let mut router = Router::default();
    let mut clip = Clip::default();
    router.event_with_clipboard(
        &mut surface,
        &Event::Key {
            scancode: 30,
            pressed: true,
            repeat: false,
        },
        &mut clip,
    )?;
    assert_eq!(surface.value("address"), Some(Value::String(String::new())));
    router.event_with_clipboard(
        &mut surface,
        &Event::TextInput {
            text: "1مرحبا🙂".into(),
        },
        &mut clip,
    )?;
    assert_eq!(
        surface.value("address"),
        Some(Value::String("1مرحبا🙂".into()))
    );
    router.event_with_clipboard(
        &mut surface,
        &Event::TextEditing {
            text: "你好".into(),
            start: 0,
            length: 2,
        },
        &mut clip,
    )?;
    assert_eq!(
        surface.value("address"),
        Some(Value::String("1مرحبا🙂".into()))
    );
    router.event_with_clipboard(
        &mut surface,
        &Event::TextInput {
            text: "你好".into(),
        },
        &mut clip,
    )?;
    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
    for key in [primary, 4, 6] {
        router.event_with_clipboard(
            &mut surface,
            &Event::Key {
                scancode: key,
                pressed: true,
                repeat: false,
            },
            &mut clip,
        )?;
    }
    assert_eq!(clip.0, "1مرحبا🙂你好");
    Ok(())
}

#[test]
fn focus_loss_releases_capture_without_clicking_or_leaking_through_pause_scope() -> Result<()> {
    let mut surface = fixture()?;
    surface.screen(Screen::Pause)?;
    let mut router = Router::default();
    let mut clip = Clip::default();
    router.sync_with_clipboard(&mut surface, &mut clip)?;
    let resume = surface.ui.nodes[surface.ui.find("resume").expect("resume")].rect;
    let point = [resume.x + resume.w * 0.5, resume.y + resume.h * 0.5];
    router.event_with_clipboard(
        &mut surface,
        &Event::MouseMotion {
            position: point,
            delta: [0., 0.],
        },
        &mut clip,
    )?;
    router.event_with_clipboard(
        &mut surface,
        &Event::MouseButton {
            button: 1,
            pressed: true,
        },
        &mut clip,
    )?;
    assert!(router.state.captured().is_some());
    router.event_with_clipboard(&mut surface, &Event::FocusLost, &mut clip)?;
    router.event_with_clipboard(
        &mut surface,
        &Event::MouseButton {
            button: 1,
            pressed: false,
        },
        &mut clip,
    )?;
    assert!(router.actions(&mut surface).is_empty());
    assert_eq!(router.state.captured(), None);
    let hud = surface.ui.nodes[surface.ui.find("hud-action").expect("hud")].rect;
    router.event_with_clipboard(
        &mut surface,
        &Event::MouseMotion {
            position: [hud.x + hud.w * 0.5, hud.y + hud.h * 0.5],
            delta: [0., 0.],
        },
        &mut clip,
    )?;
    for pressed in [true, false] {
        assert!(router.event_with_clipboard(
            &mut surface,
            &Event::MouseButton { button: 1, pressed },
            &mut clip
        )?);
    }
    assert!(
        router.actions(&mut surface).is_empty(),
        "HUD actions must remain out of reach behind Pause"
    );
    Ok(())
}
