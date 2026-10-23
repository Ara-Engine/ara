use ara_ui_text::{Clipboard, EditEvent, Result, Text, TextInput, TextSystem, inspect};
#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, s: &str) -> Result<()> {
        self.0 = s.into();
        Ok(())
    }
}
#[test]
fn editing_selection_clipboard_graphemes_ime_and_undo() -> Result<()> {
    let mut input = TextInput::new("Cafe\u{301} 👩🏽‍🚀");
    let mut clip = Clip::default();
    input.event(EditEvent::Backspace, &mut clip)?;
    assert_eq!(input.value(), "Cafe\u{301} ");
    input.event(EditEvent::Undo, &mut clip)?;
    assert_eq!(input.value(), "Cafe\u{301} 👩🏽‍🚀");
    input.event(EditEvent::SelectAll, &mut clip)?;
    input.event(EditEvent::Copy, &mut clip)?;
    input.event(EditEvent::Cut, &mut clip)?;
    assert_eq!(input.value(), "");
    input.event(EditEvent::Paste, &mut clip)?;
    assert_eq!(input.value(), clip.0);
    input.event(EditEvent::SelectAll, &mut clip)?;
    input.event(EditEvent::ImeStart, &mut clip)?;
    input.event(
        EditEvent::ImePreedit {
            text: "にほん".into(),
            selection: 0..9,
        },
        &mut clip,
    )?;
    assert_eq!(input.display(), "にほん");
    assert_ne!(input.value(), "にほん");
    assert!(
        input
            .event(
                EditEvent::ImePreedit {
                    text: "日本".into(),
                    selection: 1..6
                },
                &mut clip
            )
            .is_err()
    );
    input.event(EditEvent::ImeCancel, &mut clip)?;
    assert_eq!(input.value(), clip.0);
    input.event(EditEvent::ImeStart, &mut clip)?;
    input.event(EditEvent::ImeCommit("日本語".into()), &mut clip)?;
    assert_eq!(input.value(), "日本語");
    input.event(EditEvent::Undo, &mut clip)?;
    assert_eq!(input.value(), clip.0);
    input.event(EditEvent::Redo, &mut clip)?;
    assert_eq!(input.value(), "日本語");
    assert!(
        input
            .event(
                EditEvent::Select {
                    anchor: 1,
                    caret: 3
                },
                &mut clip
            )
            .is_err()
    );
    Ok(())
}
#[test]
fn arabic_bidi_shapes_and_reuses_paragraphs() -> Result<()> {
    let mut system = TextSystem::bundled()?;
    let mut text = Text::new("أهلاً بالعالم — Outpost 42 (جاهز)".into());
    system.measure(&mut text, Some(200.0), 16.0);
    let evidence = inspect(&text);
    assert_eq!(evidence.missing_glyphs, 0);
    assert!(evidence.rtl_runs > 0 && evidence.ltr_runs > 0);
    let before = system.shapes;
    system.measure(&mut text, Some(190.0), 16.0);
    assert_eq!(system.shapes, before);
    Ok(())
}
