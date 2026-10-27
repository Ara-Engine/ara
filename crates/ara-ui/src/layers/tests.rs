use super::*;
use crate::{input::InputEvent, source::stable_id, theme::EDITOR_CSS};
use ara_ui_text::{Clipboard, TextSystem};

struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

#[test]
fn modal_default_enter_and_escape_ignore_tooltip_layers() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='outside' text='Outside' /><modal id='dialog' default-action='accept' style='position:absolute;width:200px;height:100px'><field id='prompt' text='' /><button id='accept' text='Accept' action='accept' /></modal><tooltip id='hint' style='position:absolute;width:40px;height:20px' /></ui>",
        EDITOR_CSS,
        [400., 300.],
    )?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    input.push(
        &mut ui,
        stable_id("root"),
        Some(stable_id("outside")),
        false,
    )?;
    let mut layers = LayerStack::default();
    layers.open(
        &mut ui,
        &mut input,
        stable_id("dialog"),
        LayerKind::Modal,
        [20., 20.],
        Some(stable_id("prompt")),
    )?;
    ui.frame(&mut text)?;
    layers.event(&mut ui, &mut input, InputEvent::Activate, &mut Clip)?;
    assert_eq!(
        ui.drain_actions().next().expect("default action").name,
        "accept"
    );
    layers.open(
        &mut ui,
        &mut input,
        stable_id("hint"),
        LayerKind::Tooltip,
        [30., 30.],
        None,
    )?;
    layers.event(&mut ui, &mut input, InputEvent::Cancel, &mut Clip)?;
    assert_eq!(layers.nodes().count(), 0);
    assert_eq!(input.focused(), Some(stable_id("outside")));
    Ok(())
}

#[test]
fn removed_top_modal_restores_surviving_scope_after_reconcile() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='outside' text='Outside' /><modal id='dialog' style='width:200px;height:100px'><button id='inside' text='Inside' /></modal></ui>",
        EDITOR_CSS,
        [400., 300.],
    )?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    input.push(
        &mut ui,
        stable_id("root"),
        Some(stable_id("outside")),
        false,
    )?;
    let mut layers = LayerStack::default();
    layers.open(
        &mut ui,
        &mut input,
        stable_id("dialog"),
        LayerKind::Modal,
        [20., 20.],
        Some(stable_id("inside")),
    )?;
    ui.reconcile("<ui id='root'><button id='outside' text='Outside' /></ui>")?;
    ui.frame(&mut text)?;
    layers.sync(&mut ui, &mut input)?;
    assert_eq!(layers.nodes().count(), 0);
    assert_eq!(input.focused(), Some(stable_id("outside")));
    Ok(())
}
