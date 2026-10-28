use ara_ui_text::{Clipboard, EditEvent, Result, TextInput};
#[derive(Default)]
struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
}
#[test]
fn accepted_value_clears_draft_history_without_moving_caret_or_selection() -> Result<()> {
    let mut input = TextInput::new("abcd");
    let mut clip = Clip;
    input.event(
        EditEvent::Select {
            anchor: 1,
            caret: 3,
        },
        &mut clip,
    )?;
    input.event(EditEvent::Insert("X".into()), &mut clip)?;
    input.event(
        EditEvent::Select {
            anchor: 1,
            caret: 2,
        },
        &mut clip,
    )?;
    assert!(input.can_undo());
    let before = (input.value().to_owned(), input.anchor(), input.caret());
    assert!(input.accept_current());
    assert!(!input.can_undo() && !input.can_redo());
    assert_eq!(
        (input.value().to_owned(), input.anchor(), input.caret()),
        before
    );
    input.event(EditEvent::Insert("Y".into()), &mut clip)?;
    assert!(input.can_undo());
    input.event(EditEvent::Undo, &mut clip)?;
    assert_eq!(input.value(), before.0);
    assert!(input.can_redo());
    Ok(())
}
#[test]
fn active_ime_is_not_acknowledged_or_disturbed() -> Result<()> {
    let mut input = TextInput::new("abcd");
    let mut clip = Clip;
    input.event(
        EditEvent::Select {
            anchor: 1,
            caret: 3,
        },
        &mut clip,
    )?;
    input.event(EditEvent::Insert("X".into()), &mut clip)?;
    input.event(EditEvent::ImeStart, &mut clip)?;
    input.event(
        EditEvent::ImePreedit {
            text: "ni".into(),
            selection: 0..2,
        },
        &mut clip,
    )?;
    let before = (
        input.value().to_owned(),
        input.display(),
        input.anchor(),
        input.caret(),
        input.preedit_range(),
    );
    assert!(!input.accept_current());
    assert!(input.can_undo());
    assert_eq!(
        (
            input.value().to_owned(),
            input.display(),
            input.anchor(),
            input.caret(),
            input.preedit_range()
        ),
        before
    );
    Ok(())
}
