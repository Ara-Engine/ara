use super::*;
use crate::{source::stable_id, theme::EDITOR_CSS};
use ara_ui_text::TextSystem;

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

fn fixture() -> Result<(Ui, InputState, Clip)> {
    let mut ui = Ui::editor(
        "<ui id='root'><button id='outside' text='Outside' /><modal id='modal'><field id='first' text='' /><field id='second' text='' /><button id='accept' text='Accept' action='accept' /></modal></ui>",
        EDITOR_CSS,
        [400., 400.],
    )?;
    ui.frame(&mut TextSystem::bundled()?)?;
    let mut input = InputState::default();
    input.push(
        &mut ui,
        stable_id("root"),
        Some(stable_id("outside")),
        false,
    )?;
    input.push(&mut ui, stable_id("modal"), Some(stable_id("first")), true)?;
    Ok((ui, input, Clip::default()))
}

fn value(ui: &Ui) -> &str {
    match ui.widgets.get(&stable_id("first")).expect("field") {
        Widget::Input(text) => text.value(),
        _ => panic!("field must stay an input"),
    }
}

#[test]
fn one_text_ingress_prevents_sdl_native_echo_without_dropping_repeated_characters() -> Result<()> {
    let (mut ui, mut input, mut clip) = fixture()?;
    let mut router = Router::new(PrimaryModifier::Control, TextSource::Native);
    router.key(&mut ui, &mut input, 30, true, false, &mut clip)?;
    assert_eq!(
        value(&ui),
        "",
        "physical keys never synthesize printable text"
    );
    router.commit(&mut ui, &mut input, TextSource::Native, "你🙂", &mut clip)?;
    assert!(!router.commit(&mut ui, &mut input, TextSource::Sdl, "你🙂", &mut clip)?);
    router.commit(&mut ui, &mut input, TextSource::Native, "你🙂", &mut clip)?;
    assert_eq!(value(&ui), "你🙂你🙂");
    Ok(())
}

#[test]
fn unicode_ime_offsets_are_clamped_and_preedit_cancel_keeps_committed_text() -> Result<()> {
    let (mut ui, mut input, mut clip) = fixture()?;
    let mut router = Router::default();
    router.commit(&mut ui, &mut input, TextSource::Sdl, "أهلاً ", &mut clip)?;
    router.preedit(
        &mut ui,
        &mut input,
        TextSource::Sdl,
        "你🙂é",
        1,
        i32::MAX,
        &mut clip,
    )?;
    assert!(router.composing());
    assert_eq!(value(&ui), "أهلاً ");
    router.preedit(
        &mut ui,
        &mut input,
        TextSource::Sdl,
        "你🙂é",
        -1,
        -1,
        &mut clip,
    )?;
    assert!(router.key(&mut ui, &mut input, 41, true, false, &mut clip)?);
    assert!(!router.composing());
    assert_eq!(value(&ui), "أهلاً ");
    router.preedit(
        &mut ui,
        &mut input,
        TextSource::Sdl,
        "你好",
        0,
        2,
        &mut clip,
    )?;
    router.commit(&mut ui, &mut input, TextSource::Sdl, "你好", &mut clip)?;
    assert_eq!(value(&ui), "أهلاً 你好");
    Ok(())
}

#[test]
fn command_and_control_shortcuts_use_the_selected_primary_modifier() -> Result<()> {
    for (primary, key, other) in [
        (PrimaryModifier::Control, 224, 227),
        (PrimaryModifier::Command, 227, 224),
    ] {
        let (mut ui, mut input, mut clip) = fixture()?;
        let mut router = Router::new(primary, TextSource::Sdl);
        router.commit(&mut ui, &mut input, TextSource::Sdl, "Å🙂界", &mut clip)?;
        router.key(&mut ui, &mut input, key, true, false, &mut clip)?;
        router.key(&mut ui, &mut input, 4, true, false, &mut clip)?;
        router.key(&mut ui, &mut input, 6, true, false, &mut clip)?;
        assert_eq!(clip.0, "Å🙂界");
        router.key(&mut ui, &mut input, 27, true, false, &mut clip)?;
        assert_eq!(value(&ui), "");
        router.key(&mut ui, &mut input, 25, true, false, &mut clip)?;
        assert_eq!(value(&ui), "Å🙂界");
        router.key(&mut ui, &mut input, key, false, false, &mut clip)?;
        router.key(&mut ui, &mut input, other, true, false, &mut clip)?;
        clip.0 = "untouched".into();
        router.key(&mut ui, &mut input, 6, true, false, &mut clip)?;
        assert_eq!(
            clip.0, "untouched",
            "the other OS modifier must not invoke Copy"
        );
    }
    Ok(())
}

#[test]
fn focus_loss_cancels_pointer_and_ime_preserves_modal_and_resets_shift_tab() -> Result<()> {
    let (mut ui, mut input, mut clip) = fixture()?;
    let mut router = Router::default();
    router.key(&mut ui, &mut input, 225, true, false, &mut clip)?;
    router.key(&mut ui, &mut input, 229, true, false, &mut clip)?;
    router.key(&mut ui, &mut input, 225, false, false, &mut clip)?;
    assert!(
        router.modifiers().shift,
        "releasing left Shift preserves held right Shift"
    );
    router.preedit(
        &mut ui,
        &mut input,
        TextSource::Sdl,
        "未提交",
        0,
        3,
        &mut clip,
    )?;
    let rect = ui.nodes[ui.find("accept").expect("button")].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    input.event(&mut ui, InputEvent::PointerDown(point), &mut clip)?;
    assert!(input.captured().is_some());
    router.focus_lost(&mut ui, &mut input, &mut clip)?;
    input.event(&mut ui, InputEvent::PointerUp(point), &mut clip)?;
    assert_eq!(input.captured(), None);
    assert!(
        ui.drain_actions().next().is_none(),
        "release after focus loss cannot click"
    );
    assert!(
        input.focus(&mut ui, stable_id("outside")).is_err(),
        "focus loss must preserve the modal scope"
    );
    assert_eq!(value(&ui), "");
    router.key(&mut ui, &mut input, 43, true, false, &mut clip)?;
    assert_eq!(
        input.focused(),
        Some(stable_id("first")),
        "Tab is forward after lost Shift release"
    );
    Ok(())
}
