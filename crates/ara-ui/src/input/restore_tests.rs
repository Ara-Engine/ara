//! Deferred host restoration must never steal a nested decision's focus/capture.
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
fn markup(target: &str) -> String {
    format!(
        "<ui id='root'><button id='base' text='Base' style='width:120px;height:28px'/>{target}<panel id='modal' style='position:absolute;left:180px;top:50px;width:220px;height:180px'><button id='modal-control' text='Choose' style='width:160px;height:28px'/><panel id='popup' style='position:absolute;left:0px;top:40px;width:180px;height:100px'><button id='popup-control' text='Option' style='width:160px;height:28px'/></panel></panel></ui>"
    )
}
const TARGET: &str =
    "<field id='rename' text='Uncommitted draft' style='width:120px;height:28px'/>";

#[test]
fn background_restore_preserves_nested_scope_capture_and_retries_after_the_scope_closes()
-> Result<()> {
    let mut ui = Ui::editor(&markup(TARGET), EDITOR_CSS, [640., 480.])?;
    let mut text = TextSystem::bundled()?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    input.focus(&mut ui, stable_id("base"))?;
    input.push(
        &mut ui,
        stable_id("modal"),
        Some(stable_id("modal-control")),
        true,
    )?;
    input.push(
        &mut ui,
        stable_id("popup"),
        Some(stable_id("popup-control")),
        false,
    )?;
    let rect = ui.nodes[ui.find("popup-control").ok_or("popup control")?].rect;
    input.event(
        &mut ui,
        InputEvent::PointerDown([rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]),
        &mut Clip,
    )?;
    let focus = input.focused();
    let capture = input.captured();
    assert_eq!(capture, Some(stable_id("popup-control")));
    for _ in 0..8 {
        assert!(!input.focus_if_available(&mut ui, stable_id("rename")));
        assert!(!input.focus_if_available(&mut ui, stable_id("not-mounted")));
        assert_eq!(input.focused(), focus);
        assert_eq!(input.captured(), capture);
        assert_eq!(input.scopes.len(), 2);
    }
    assert!(
        input.focus(&mut ui, stable_id("rename")).is_err(),
        "explicit invalid focus remains ARA4045"
    );
    assert!(input.remove_scope(&mut ui, stable_id("modal")));
    assert!(!input.focus_if_available(&mut ui, stable_id("rename")));
    assert_eq!(input.captured(), capture);
    input.pop(&mut ui);
    assert!(input.focus_if_available(&mut ui, stable_id("rename")));
    assert_eq!(input.focused(), Some(stable_id("rename")));
    assert!(input.scopes.is_empty());
    Ok(())
}

#[test]
fn unavailable_restore_does_not_scroll_or_consume_the_hosts_intent() -> Result<()> {
    let mut text = TextSystem::bundled()?;
    for target in [
        "",
        "<field id='rename' disabled='true' text='Draft' style='width:120px;height:28px'/>",
        "<field id='rename' style='display:none;width:120px;height:28px' text='Draft'/>",
        "<scroll id='scroll' style='width:140px;height:50px'><panel style='height:200px;flex-shrink:0'/><field id='rename' text='Draft' style='height:28px;flex-shrink:0'/></scroll>",
    ] {
        let mut ui = Ui::editor(&markup(target), EDITOR_CSS, [640., 480.])?;
        ui.frame(&mut text)?;
        let mut input = InputState::default();
        input.focus(&mut ui, stable_id("base"))?;
        let scroll = ui
            .find("scroll")
            .and_then(|index| ui.scroll_position(index));
        assert!(!input.focus_if_available(&mut ui, stable_id("rename")));
        assert_eq!(input.focused(), Some(stable_id("base")));
        assert_eq!(
            ui.find("scroll")
                .and_then(|index| ui.scroll_position(index)),
            scroll
        );
        ui.reconcile(&markup(TARGET))?;
        ui.frame(&mut text)?;
        assert!(input.focus_if_available(&mut ui, stable_id("rename")));
        assert_eq!(input.focused(), Some(stable_id("rename")));
    }
    Ok(())
}
