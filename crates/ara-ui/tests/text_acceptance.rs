use ara_ui::{Ui, input::InputState, native_input::Router, source::stable_id, widgets::Widget};
use ara_ui_text::{Clipboard, EditEvent, Result, TextSystem};
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
fn draft_echo_keeps_local_undo_but_explicit_acceptance_yields_empty_history_to_host() -> Result<()>
{
    let mut ui = Ui::parse(
        "<ui id='r'><field id='f' text='old' /></ui>",
        "",
        [300., 100.],
    )?;
    let mut system = TextSystem::bundled()?;
    ui.frame(&mut system)?;
    let mut input = InputState::default();
    input.focus(&mut ui, stable_id("f"))?;
    let mut router = Router::default();
    let mut clip = Clip;
    let index = ui.find("f").expect("field");
    ui.edit(index, EditEvent::SelectAll, &mut clip)?;
    ui.edit(index, EditEvent::Insert("draft".into()), &mut clip)?;
    ui.reconcile("<ui id='r'><field id='f' text='draft' /></ui>")?;
    assert!(matches!(ui.widgets.get(&stable_id("f")),Some(Widget::Input(text)) if text.can_undo()));
    let index = ui.find("f").expect("same field");
    assert!(ui.accept_text(index)?);
    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
    router.key(&mut ui, &mut input, primary, true, false, &mut clip)?;
    assert!(!router.key(&mut ui, &mut input, 29, true, false, &mut clip)?);
    assert!(!router.key(&mut ui, &mut input, 28, true, false, &mut clip)?);
    assert_eq!(input.focused(), Some(stable_id("f")));
    Ok(())
}
