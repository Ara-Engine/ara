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
        // Pushing a scope focuses its first control; fields start idle.
        input.blur(&mut ui);
        let rect = ui.nodes[ui.find("number").unwrap()].rect;
        Self {
            ui,
            input,
            text,
            clipboard: Clip::default(),
            start: [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
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

fn strings(actions: Vec<crate::binding::Action>) -> Vec<String> {
    actions
        .into_iter()
        .map(|action| action.value.as_str().unwrap_or("<number>").to_owned())
        .collect()
}

#[test]
fn dragging_anywhere_previews_text_then_commits_one_number_and_one_undo() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    assert!(f.input.scrubbing());
    let end = [f.start[0] + 14., f.start[1]];
    f.event(InputEvent::PointerMove(end));
    assert_eq!(f.value(), "1.25");
    assert_eq!(f.display(), "3.00");
    // Hosts preview text drafts live while the pointer moves.
    assert_eq!(strings(f.ui.drain_actions().collect()), ["3.00"]);
    f.event(InputEvent::PointerUp(end));
    let actions: Vec<_> = f.ui.drain_actions().collect();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].value, serde_json::json!(3.));
    assert_eq!(f.value(), "3.00");
    assert!(!f.input.scrubbing());
    // A finished drag leaves the field ready for the next drag, not for typing.
    assert_eq!(f.input.focused(), None);
    f.input.focus(&mut f.ui, stable_id("number")).unwrap();
    f.event(InputEvent::Edit(EditEvent::Undo));
    assert_eq!(f.value(), "1.25");
}

#[test]
fn a_click_edits_and_escape_restores_the_previewed_value() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerUp([f.start[0] + 2., f.start[1]]));
    assert_eq!(f.value(), "1.25");
    assert_eq!(f.ui.drain_actions().count(), 0);
    assert_eq!(f.input.focused(), Some(stable_id("number")));
    let Some(Widget::Input(input)) = f.ui.widgets.get(&stable_id("number")) else {
        panic!("fixture text input")
    };
    assert_eq!(input.selection().len(), 4, "the whole value is selected");
    f.input.blur(&mut f.ui);
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    assert_ne!(f.display(), "1.25");
    f.event(InputEvent::Cancel);
    assert_eq!(f.value(), "1.25");
    assert_eq!(f.display(), "1.25");
    // The host saw a preview, so it is told the original again.
    assert_eq!(strings(f.ui.drain_actions().collect()), ["3.00", "1.25"]);
    assert_eq!(f.input.captured(), None);
}

#[test]
fn a_field_being_edited_takes_the_caret_on_click_and_still_scrubs_on_drag() {
    let mut f = Fixture::new();
    assert_eq!(f.ui.cursor(&f.input, f.start), Cursor::ResizeEw);
    f.input.focus(&mut f.ui, stable_id("number")).unwrap();
    f.ui.frame(&mut f.text).unwrap();
    assert_eq!(f.ui.cursor(&f.input, f.start), Cursor::Text);
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerUp(f.start));
    let Some(Widget::Input(input)) = f.ui.widgets.get(&stable_id("number")) else {
        panic!("fixture text input")
    };
    assert!(input.selection().is_empty(), "a click places the caret");
    assert_eq!(f.input.focused(), Some(stable_id("number")));
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    assert!(f.input.scrubbing());
    assert_eq!(f.display(), "3.00");
}

#[test]
fn slider_fields_fill_to_their_value_and_span_their_range_across_the_width() {
    let mut text = TextSystem::bundled().unwrap();
    let markup = "<ui id='root' style='width:100%;height:100%;align-items:start'><field id='number' class='number slider' text='1' step='0.01' min='0' max='4' action='Changed' style='width:200px' /></ui>";
    let mut ui = Ui::editor(markup, EDITOR_CSS, [400., 160.]).unwrap();
    ui.frame(&mut text).unwrap();
    let element = &ui.nodes[ui.find("number").unwrap()].element;
    assert_eq!(slider_ratio(element, "1"), Some(0.25));
    assert_eq!(slider_ratio(element, "9"), Some(1.));
    let mut input = InputState::default();
    input.push(&mut ui, stable_id("root"), None, false).unwrap();
    input.blur(&mut ui);
    let rect = ui.nodes[ui.find("number").unwrap()].rect;
    let start = [rect.x + 50., rect.y + rect.h * 0.5];
    let mut clipboard = Clip::default();
    input
        .event(&mut ui, InputEvent::PointerDown(start), &mut clipboard)
        .unwrap();
    // Past the 4 px threshold, 50 px of a 200 px slider is a quarter of its range.
    let end = [start[0] + 54., start[1]];
    input
        .event(&mut ui, InputEvent::PointerUp(end), &mut clipboard)
        .unwrap();
    let last = ui.drain_actions().last().unwrap();
    assert_eq!(last.value, serde_json::json!(2.));
}

#[test]
fn blur_and_host_reconciliation_cancel_without_overwriting_new_data() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.input.blur(&mut f.ui);
    assert!(!f.input.scrubbing());
    assert_eq!(f.display(), "1.25");
    f.ui.drain_actions().count();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.ui.reconcile(&markup("2.5")).unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerUp(f.start));
    assert_eq!(f.value(), "2.5");
    assert_eq!(f.display(), "2.5");
    // Only the preview and the restore of the original it replaced.
    assert_eq!(strings(f.ui.drain_actions().collect()), ["3.00", "1.25"]);
    f.event(InputEvent::PointerDown(f.start));
    f.ui.reconcile("<ui id='root'><button id='next' text='Survives' /></ui>")
        .unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerUp(f.start));
    assert_eq!(f.input.captured(), None);
}

#[test]
fn a_host_echo_of_the_preview_keeps_the_drag_alive() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 8., f.start[1]]));
    let preview = f.display().to_owned();
    f.ui.reconcile(&markup(&preview)).unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerMove([f.start[0] + 10., f.start[1]]));
    assert!(f.input.scrubbing());
}

#[test]
fn nonfinite_coordinates_cancel_preview_and_never_emit_nan() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.ui.drain_actions().count();
    let result = f.input.event(
        &mut f.ui,
        InputEvent::PointerMove([f32::NAN, 0.]),
        &mut f.clipboard,
    );
    assert!(result.is_err());
    assert!(!f.input.scrubbing());
    assert_eq!(f.display(), "1.25");
    assert!(
        f.ui.drain_actions()
            .all(|action| action.value.as_f64().is_none_or(f64::is_finite))
    );
}

#[test]
fn reconciliation_can_remove_the_field_text_during_a_scrub() {
    let mut f = Fixture::new();
    f.event(InputEvent::PointerDown(f.start));
    f.event(InputEvent::PointerMove([f.start[0] + 12., f.start[1]]));
    f.ui.drain_actions().count();
    f.ui.reconcile("<ui id='root'><field id='number' class='number' /></ui>")
        .unwrap();
    f.ui.frame(&mut f.text).unwrap();
    f.event(InputEvent::PointerUp(f.start));
    assert!(!f.input.scrubbing());
    assert!(f.ui.drain_actions().all(|action| !action.value.is_number()));
}
