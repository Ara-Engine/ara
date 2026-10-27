//! Actual retained input events cover gesture ownership, commits and cancellation.
#![allow(clippy::unwrap_used)] // Tests assert fixture operations directly.
use super::*;
use crate::{cursor::Cursor, theme::EDITOR_CSS};
use ara_ui_text::TextSystem;

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}

fn markup(value: &str) -> String {
    format!(
        "<ui id='root' style='width:100%;height:100%;align-items:start'><field id='number' class='number' text='{value}' step='0.25' min='0' max='3' action='Changed' style='width:180px' /><button id='next' text='Next' /></ui>"
    )
}

struct Fixture {
    ui: Ui,
    input: InputState,
    text: TextSystem,
    clipboard: Clip,
    start: [f32; 2],
}
impl Fixture {
    fn new() -> Self {
        let mut text = TextSystem::bundled().unwrap();
        let mut ui = Ui::editor(&markup("1.25"), EDITOR_CSS, [400., 160.]).unwrap();
        ui.frame(&mut text).unwrap();
        let mut input = InputState::default();
        input.push(&mut ui, stable_id("root"), None, false).unwrap();
        let rect = ui.nodes[ui.find("number").unwrap()].rect;
        Self {
            ui,
            input,
            text,
            clipboard: Clip::default(),
            start: [rect.x + rect.w - 8., rect.y + rect.h * 0.5],
        }
    }
    fn event(&mut self, event: InputEvent) {
        self.input
            .event(&mut self.ui, event, &mut self.clipboard)
            .unwrap();
        self.ui.frame(&mut self.text).unwrap();
    }
    fn value(&self) -> &str {
        let Some(Widget::Input(input)) = self.ui.widgets.get(&stable_id("number")) else {
            panic!("fixture text input")
        };
        input.value()
    }
    fn display(&self) -> &str {
        &self.ui.nodes[self.ui.find("number").unwrap()]
            .text
            .as_ref()
            .unwrap()
            .value
    }
}

#[test]
fn numeric_grip_previews_then_commits_one_bounded_action_and_one_undo() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    assert!(f.input.scrubbing());
    let end = [f.start[0] + 14., f.start[1]];
    f.event(InputEvent::PointerMove(end));
    assert_eq!(f.value(), "1.25");
    assert_eq!(f.display(), "3.00");
    assert_eq!(f.ui.drain_actions().count(), 0);
    f.event(InputEvent::PointerUp(end));
    let actions: Vec<_> = f.ui.drain_actions().collect();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].value, serde_json::json!(3.));
    assert_eq!(f.value(), "3.00");
    assert!(!f.input.scrubbing());
    f.event(InputEvent::Edit(EditEvent::Undo));
    assert_eq!(f.value(), "1.25");
}

#[test]
fn threshold_and_escape_leave_the_authoring_value_untouched() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    let near = [f.start[0] + 5., f.start[1]];
    f.event(InputEvent::PointerUp(near));
    assert_eq!(f.value(), "1.25");
    assert_eq!(f.ui.drain_actions().count(), 0);
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    assert_ne!(f.display(), "1.25");
    f.event(InputEvent::Cancel);
    assert_eq!(f.value(), "1.25");
    assert_eq!(f.display(), "1.25");
    assert_eq!(f.ui.drain_actions().count(), 0);
    assert_eq!(f.input.captured(), None);
}

#[test]
fn text_area_keeps_selection_and_only_the_grip_uses_a_scrub_cursor() {
    let mut f = Fixture::new();
    let rect = f.ui.nodes[f.ui.find("number").unwrap()].rect;
    let text = [rect.x + 9., f.start[1]];
    assert_eq!(f.ui.cursor(&f.input, text), Cursor::Text);
    assert_eq!(f.ui.cursor(&f.input, f.start), Cursor::ResizeEw);
    f.event(InputEvent::PointerDown(text));
    f.event(InputEvent::PointerMove([text[0] + 15., text[1]]));
    assert!(!f.input.scrubbing());
    let Some(Widget::Input(input)) = f.ui.widgets.get(&stable_id("number")) else {
        panic!("fixture text input")
    };
    assert!(!input.selection().is_empty());
    assert_eq!(f.value(), "1.25");
}

#[test]
fn blur_and_host_reconciliation_cancel_without_overwriting_new_data() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.input.blur(&mut f.ui);
    assert!(!f.input.scrubbing());
    assert_eq!(f.display(), "1.25");
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.ui.reconcile(&markup("2.5")).unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerUp(f.start));
    assert_eq!(f.value(), "2.5");
    assert_eq!(f.display(), "2.5");
    assert_eq!(f.ui.drain_actions().count(), 0);
    f.event(InputEvent::PointerDown(f.start));
    f.ui.reconcile("<ui id='root'><button id='next' text='Survives' /></ui>")
        .unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerUp(f.start));
    assert_eq!(f.input.captured(), None);
}

#[test]
fn nonfinite_coordinates_cancel_preview_and_never_emit_nan() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    let result = f.input.event(
        &mut f.ui,
        InputEvent::PointerMove([f32::NAN, 0.]),
        &mut f.clipboard,
    );
    assert!(result.is_err());
    assert!(!f.input.scrubbing());
    assert_eq!(f.display(), "1.25");
    assert_eq!(f.ui.drain_actions().count(), 0);
}

#[test]
fn reconciliation_can_remove_the_field_text_during_a_scrub() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.ui.reconcile("<ui id='root'><field id='number' class='number' /></ui>")
        .unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerUp(f.start));
    assert!(!f.input.scrubbing());
    assert_eq!(f.ui.drain_actions().count(), 0);
}
