use super::super::CSS;
use super::*;
use crate::{
    Result, Ui,
    input::{InputEvent, InputState},
};
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

fn surface(size: [f32; 2], host: &str) -> Result<(Ui, TextSystem, InputState)> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='outside' text='Outside' /></ui>",
        &format!("{}{host}{CSS}", crate::theme::EDITOR_CSS),
        size,
    )?;
    ui.frame(&mut text)?;
    let mut input = InputState::default();
    input.push(
        &mut ui,
        stable_id("root"),
        Some(stable_id("outside")),
        false,
    )?;
    Ok((ui, text, input))
}
fn mount(
    dialogs: &mut Dialogs,
    ui: &mut Ui,
    text: &mut TextSystem,
    input: &mut InputState,
) -> Result<()> {
    ui.reconcile(&format!(
        "<ui id='root'><button id='outside' text='Outside' />{}</ui>",
        dialogs.markup(ui.size)
    ))?;
    ui.frame(text)?;
    dialogs.sync(ui, input)?;
    ui.frame(text)?;
    Ok(())
}

#[test]
fn empty_choices_complete_without_ui_and_out_of_range_default_is_safe() -> Result<()> {
    let (mut ui, mut text, mut input) = surface([480., 320.], "")?;
    let mut dialogs = Dialogs::default();
    let mut empty = dialogs.choose(
        DialogOptions::new("No conflicts", "Nothing needs a choice."),
        Vec::new(),
        usize::MAX,
    );
    assert_eq!(empty.try_take(), Some(None));
    assert!(dialogs.is_empty());
    let mut choice = dialogs.choose(
        DialogOptions::new("Conflict", "Choose an action."),
        vec!["Rename".into(), "Skip".into()],
        usize::MAX,
    );
    mount(&mut dialogs, &mut ui, &mut text, &mut input)?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-1-choice-1")));
    dialogs.event(&mut ui, &mut input, InputEvent::Activate, &mut Clip)?;
    assert_eq!(choice.try_take(), Some(Some(1)));
    assert_eq!(input.focused(), Some(stable_id("outside")));
    let mut rename = dialogs.choose(
        DialogOptions::new("Existing file", "Rename is the safe default."),
        vec!["Rename".into(), "Replace".into(), "Skip".into()],
        0,
    );
    mount(&mut dialogs, &mut ui, &mut text, &mut input)?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-2-choice-0")));
    dialogs.event(&mut ui, &mut input, InputEvent::Activate, &mut Clip)?;
    assert_eq!(rename.try_take(), Some(Some(0)));
    let mut dropped = dialogs.choose(
        DialogOptions::new("Close", "Dropping a controller cancels the request."),
        vec!["Rename".into()],
        0,
    );
    drop(dialogs);
    assert_eq!(dropped.try_take(), Some(None));
    Ok(())
}

#[test]
fn stacked_choices_return_the_focused_action_and_escape_restores_the_pending_prompt() -> Result<()>
{
    let (mut ui, mut text, mut input) = surface([480., 400.], "")?;
    let mut dialogs = Dialogs::default();
    let mut prompt = dialogs.prompt(
        DialogOptions::new("Name", "Keep this draft while resolving a conflict."),
        "Draft",
    );
    mount(&mut dialogs, &mut ui, &mut text, &mut input)?;
    let labels = || vec!["Rename".into(), "Replace".into(), "Skip".into()];
    let mut choice = dialogs.choose(
        DialogOptions::new("Existing file", "Choose how to resolve the conflict."),
        labels(),
        0,
    );
    mount(&mut dialogs, &mut ui, &mut text, &mut input)?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-2-choice-0")));
    assert!(input.focus(&mut ui, stable_id("outside")).is_err());
    dialogs.event(
        &mut ui,
        &mut input,
        InputEvent::Tab { reverse: false },
        &mut Clip,
    )?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-2-choice-1")));
    dialogs.event(&mut ui, &mut input, InputEvent::Activate, &mut Clip)?;
    assert_eq!(choice.try_take(), Some(Some(1)));
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-1-field")));
    let mut cancelled = dialogs.choose(
        DialogOptions::new("Another file", "Escape cancels this request."),
        labels(),
        0,
    );
    mount(&mut dialogs, &mut ui, &mut text, &mut input)?;
    dialogs.event(&mut ui, &mut input, InputEvent::Cancel, &mut Clip)?;
    assert_eq!(cancelled.try_take(), Some(None));
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-1-field")));
    dialogs.event(&mut ui, &mut input, InputEvent::Activate, &mut Clip)?;
    assert_eq!(prompt.try_take(), Some(Some("Draft".into())));
    assert_eq!(input.focused(), Some(stable_id("outside")));
    Ok(())
}

#[test]
fn localized_choices_reflow_under_host_resets_and_remain_pointer_reachable() -> Result<()> {
    let (mut ui, mut text, mut input) = surface(
        [480., 480.],
        "scroll { flex-basis:0px; min-height:0px; } button { height:22px; }",
    )?;
    let mut dialogs = Dialogs::default();
    let options = DialogOptions::new(
        "Resolve existing files",
        "Read these recovery notes before choosing. ".repeat(12),
    );
    let labels=(0..3).map(|index|format!("Action {index}: conserver les modifications, إعدادات المشروع, and return safely to the current project")).collect();
    let mut response = dialogs.choose(options, labels, 0);
    mount(&mut dialogs, &mut ui, &mut text, &mut input)?;
    for _ in 0..2 {
        dialogs.event(
            &mut ui,
            &mut input,
            InputEvent::Tab { reverse: false },
            &mut Clip,
        )?;
    }
    ui.frame(&mut text)?;
    let body = &ui.nodes[ui.find("ara-dialog-1-body").expect("body")];
    assert!(
        body.rect.y >= 0. && body.rect.y + body.rect.h <= 480.5,
        "{:?}",
        body.rect
    );
    let option = ui.find("ara-dialog-1-choice-2").expect("third choice");
    let node = &ui.nodes[option];
    let layout = ui.tree.layout(node.layout_id)?;
    let content_height = layout.size.height
        - layout.padding.top
        - layout.padding.bottom
        - layout.border.top
        - layout.border.bottom;
    assert!(node.text.as_ref().expect("label").layout.height() <= content_height + 0.5);
    let point = [
        node.rect.x + node.rect.w * 0.5,
        node.rect.y + node.rect.h * 0.5,
    ];
    assert!(
        node.clip.h > 0. && node.clip.contains(point[0], point[1]),
        "{:?} clipped by {:?}",
        node.rect,
        node.clip
    );
    assert_eq!(ui.hit_test(point[0], point[1]), Some(option));
    dialogs.event(
        &mut ui,
        &mut input,
        InputEvent::PointerDown(point),
        &mut Clip,
    )?;
    dialogs.event(&mut ui, &mut input, InputEvent::PointerUp(point), &mut Clip)?;
    assert_eq!(response.try_take(), Some(Some(2)));
    assert_eq!(input.focused(), Some(stable_id("outside")));
    ui.frame(&mut text)?;
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    Ok(())
}
