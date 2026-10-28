//! First overlays restore unscoped host focus rather than selecting the first control.
use super::*;
use crate::theme::EDITOR_CSS;
use ara_ui_text::TextSystem;

struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

fn markup(return_control: &str) -> String {
    format!(
        "<ui id='root' style='gap:6px'><button id='first' text='First fallback' style='width:180px;height:28px;padding:0px' />{return_control}<modal id='lower' style='position:absolute;left:220px;top:90px;width:280px;height:100px'><button id='lower-button' text='Lower' style='height:28px;padding:0px' /></modal><modal id='upper' style='position:absolute;left:260px;top:220px;width:280px;height:100px'><button id='upper-button' text='Upper' style='height:28px;padding:0px' /></modal></ui>"
    )
}
const RETURN: &str =
    "<field id='return' text='Draft survives' style='width:180px;height:28px;padding:0px' />";

#[test]
fn removing_an_underlying_first_scope_preserves_top_capture_and_unscoped_focus_or_none()
-> Result<()> {
    let mut text = TextSystem::bundled()?;
    for focused in [true, false] {
        let mut ui = Ui::editor(&markup(RETURN), EDITOR_CSS, [640., 480.])?;
        ui.frame(&mut text)?;
        let mut input = InputState::default();
        if focused {
            let target = ui.nodes[ui.find("return").ok_or("Return field missing")?].rect;
            let point = [target.x + target.w * 0.5, target.y + target.h * 0.5];
            input.event(&mut ui, InputEvent::PointerDown(point), &mut Clip)?;
            input.event(&mut ui, InputEvent::PointerUp(point), &mut Clip)?;
        }
        let expected = input.focused();
        assert_eq!(expected, focused.then_some(stable_id("return")));
        input.push(
            &mut ui,
            stable_id("lower"),
            Some(stable_id("lower-button")),
            true,
        )?;
        input.push(
            &mut ui,
            stable_id("upper"),
            Some(stable_id("upper-button")),
            true,
        )?;
        let target = ui.nodes[ui.find("upper-button").ok_or("Upper button missing")?].rect;
        let point = [target.x + target.w * 0.5, target.y + target.h * 0.5];
        input.event(&mut ui, InputEvent::PointerDown(point), &mut Clip)?;
        let capture = input.captured();
        assert_eq!(capture, Some(stable_id("upper-button")));
        ui.set_visible(ui.find("lower").ok_or("Lower modal missing")?, false)?;
        assert!(input.remove_scope(&mut ui, stable_id("lower")));
        assert_eq!(input.focused(), Some(stable_id("upper-button")));
        assert_eq!(input.captured(), capture);
        ui.set_visible(ui.find("upper").ok_or("Upper modal missing")?, false)?;
        assert!(input.remove_scope(&mut ui, stable_id("upper")));
        assert_eq!(input.focused(), expected);
        assert!(input.captured().is_none());
        assert!(input.scopes.is_empty());
        assert!(
            input.unscoped_focus.is_none(),
            "A finished stack must not leak its return target"
        );
    }
    Ok(())
}

#[test]
fn vanished_disabled_or_clipped_unscoped_return_target_uses_a_surviving_fallback() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    for return_control in [
        "",
        "<field id='return' disabled='true' text='Draft survives' style='width:180px;height:28px;padding:0px' />",
        "<field id='return' text='Draft survives' style='position:absolute;left:0px;top:600px;width:180px;height:28px;padding:0px' />",
    ] {
        let mut ui = Ui::editor(&markup(RETURN), EDITOR_CSS, [640., 480.])?;
        ui.frame(&mut text)?;
        let mut input = InputState::default();
        input.focus(&mut ui, stable_id("return"))?;
        input.push(
            &mut ui,
            stable_id("lower"),
            Some(stable_id("lower-button")),
            true,
        )?;
        ui.reconcile(&markup(return_control))?;
        ui.frame(&mut text)?;
        ui.set_visible(ui.find("lower").ok_or("Lower modal missing")?, false)?;
        ui.set_visible(ui.find("upper").ok_or("Upper modal missing")?, false)?;
        assert!(input.remove_scope(&mut ui, stable_id("lower")));
        assert_eq!(input.focused(), Some(stable_id("first")));
        input.event(&mut ui, InputEvent::Activate, &mut Clip)?;
        assert_eq!(input.focused(), Some(stable_id("first")));
    }
    Ok(())
}
