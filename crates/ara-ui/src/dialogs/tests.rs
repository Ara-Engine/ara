use super::*;
use crate::{theme::EDITOR_CSS, widgets::Widget};
use ara_ui_text::{Clipboard, EditEvent, TextSystem};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};

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

fn poll<T>(future: &mut DialogFuture<T>) -> Poll<T> {
    Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
}

fn mount(dialogs: &Dialogs, ui: &mut Ui, text: &mut TextSystem) -> Result<()> {
    ui.reconcile(&format!(
        "<ui id='root'><button id='outside' text='Outside' />{}</ui>",
        dialogs.markup(ui.size)
    ))?;
    ui.frame(text)?;
    Ok(())
}

fn surface() -> Result<(Ui, TextSystem, InputState)> {
    surface_with_styles([480., 320.], "")
}

fn surface_with_styles(size: [f32; 2], host_styles: &str) -> Result<(Ui, TextSystem, InputState)> {
    let mut text = TextSystem::bundled()?;
    let mut ui = Ui::editor(
        "<ui id='root'><button id='outside' text='Outside' /></ui>",
        &format!("{EDITOR_CSS}{host_styles}{CSS}"),
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

#[test]
fn prompt_uses_real_text_events_enter_and_restores_focus() -> Result<()> {
    let (mut ui, mut text, mut input) = surface()?;
    let mut dialogs = Dialogs::default();
    let mut future = dialogs.prompt(DialogOptions::new("Save layout", "Name this layout."), "");
    assert!(poll(&mut future).is_pending());
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-1-field")));
    let mut clip = Clip::default();
    dialogs.event(
        &mut ui,
        &mut input,
        InputEvent::Edit(EditEvent::Insert("Workspace".into())),
        &mut clip,
    )?;
    // A whole host rebuild must preserve the prompt's keyboard scope and draft.
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-1-field")));
    dialogs.event(&mut ui, &mut input, InputEvent::Activate, &mut clip)?;
    assert_eq!(poll(&mut future), Poll::Ready(Some("Workspace".into())));
    assert_eq!(input.focused(), Some(stable_id("outside")));
    assert!(dialogs.is_empty());
    assert!(!ui.shown(ui.find("ara-dialog-1").expect("mounted dialog")));
    Ok(())
}

#[test]
fn nested_destructive_confirmation_traps_focus_and_escape_returns_to_prompt() -> Result<()> {
    let (mut ui, mut text, mut input) = surface()?;
    let mut dialogs = Dialogs::default();
    let mut prompt = dialogs.prompt(DialogOptions::new("Name", "Set a name."), "");
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    let mut options = DialogOptions::new("Delete?", "This cannot be undone.");
    options.destructive = true;
    options.accept_label = "Delete".into();
    let mut confirm = dialogs.confirm(options);
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-2-cancel")));
    let mut clip = Clip::default();
    for _ in 0..8 {
        dialogs.event(
            &mut ui,
            &mut input,
            InputEvent::Tab { reverse: false },
            &mut clip,
        )?;
        assert!(
            matches!(input.focused(), Some(id) if id == stable_id("ara-dialog-2-cancel") || id == stable_id("ara-dialog-2-accept"))
        );
    }
    dialogs.event(
        &mut ui,
        &mut input,
        InputEvent::PointerDown([1., 1.]),
        &mut clip,
    )?;
    assert_eq!(
        dialogs.len(),
        2,
        "destructive backdrop must not accidentally confirm/dismiss"
    );
    dialogs.event(&mut ui, &mut input, InputEvent::Cancel, &mut clip)?;
    assert_eq!(poll(&mut confirm), Poll::Ready(false));
    assert_eq!(input.focused(), Some(stable_id("ara-dialog-1-field")));
    // Empty accepted input differs from cancellation.
    dialogs.event(&mut ui, &mut input, InputEvent::Activate, &mut clip)?;
    assert_eq!(poll(&mut prompt), Poll::Ready(Some(String::new())));
    Ok(())
}

#[test]
fn controller_drop_resolves_waiters_and_owned_dispatch_preserves_host_actions() -> Result<()> {
    let (mut ui, mut text, mut input) = surface()?;
    let mut dialogs = Dialogs::default();
    let mut future = dialogs.confirm(DialogOptions::new("Continue?", "Confirm this action."));
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    ui.actions.push(crate::binding::Action {
        name: "host.keep".into(),
        node: stable_id("outside"),
        value: serde_json::Value::Null,
    });
    dialogs.dispatch(&mut ui, &mut input)?;
    assert_eq!(
        ui.drain_actions()
            .next()
            .expect("preserved host action")
            .name,
        "host.keep"
    );
    drop(dialogs);
    assert_eq!(poll(&mut future), Poll::Ready(false));
    Ok(())
}

#[test]
fn dialog_wraps_and_scrolls_in_a_small_viewport() -> Result<()> {
    let (mut ui, mut text, mut input) = surface()?;
    ui.resize([240., 180.])?;
    let mut dialogs = Dialogs::default();
    let _future = dialogs.alert(DialogOptions::new(
        "A long diagnostic",
        "A wrapping diagnostic with recovery instructions. ".repeat(35),
    ));
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    let body = &ui.nodes[ui.find("ara-dialog-1-body").expect("body")];
    assert!(
        body.rect.w <= 224.5 && body.rect.y + body.rect.h <= 172.5,
        "{:?}",
        body.rect
    );
    let accept = &ui.nodes[ui.find("ara-dialog-1-accept").expect("accept")];
    assert!(accept.rect.y + accept.rect.h <= 180.5, "{:?}", accept.rect);
    assert!(matches!(
        ui.widgets.get(&stable_id("ara-dialog-1-scroll")),
        Some(Widget::Scroll(_))
    ));
    Ok(())
}

#[test]
fn host_scroll_and_button_resets_keep_long_notes_reachable_and_action_text_fitted() -> Result<()> {
    let (mut ui, mut text, mut input) = surface_with_styles(
        [280., 260.],
        "scroll { flex-basis:0px; min-height:0px; } button { height:22px; }",
    )?;
    let mut dialogs = Dialogs::default();
    let mut options = DialogOptions::new(
        "A long diagnostic",
        "Detailed recovery instructions with éclairage and إعدادات العرض. ".repeat(40),
    );
    options.accept_label = "Keep changes and return safely".into();
    options.cancel_label = "Cancel and keep this project open".into();
    let _future = dialogs.confirm(options);
    mount(&dialogs, &mut ui, &mut text)?;
    dialogs.sync(&mut ui, &mut input)?;
    ui.frame(&mut text)?;
    let body = &ui.nodes[ui.find("ara-dialog-1-body").expect("dialog body")];
    assert!(
        body.rect.y >= 0. && body.rect.y + body.rect.h <= 260.5,
        "{:?}",
        body.rect
    );
    let scroll = ui.find("ara-dialog-1-scroll").expect("notes scroll");
    let visible = ui.nodes[scroll].rect.intersection(ui.nodes[scroll].clip);
    assert!(
        visible.h > 0.,
        "long notes need a positive visible scroll area: {visible:?}"
    );
    for id in ["ara-dialog-1-cancel", "ara-dialog-1-accept"] {
        let node = &ui.nodes[ui.find(id).expect("action")];
        let layout = ui.tree.layout(node.layout_id)?;
        let available = layout.size.height
            - layout.padding.top
            - layout.padding.bottom
            - layout.border.top
            - layout.border.bottom;
        let paragraph = node.text.as_ref().expect("action label");
        assert!(paragraph.layout.height() > 22., "action label wraps");
        assert!(
            paragraph.layout.height() <= available + 0.5,
            "{id}: {} > {available}",
            paragraph.layout.height()
        );
        assert!(
            node.rect.y + node.rect.h <= 260.5,
            "action remains reachable"
        );
    }
    dialogs.event(
        &mut ui,
        &mut input,
        InputEvent::Wheel {
            position: [visible.x + visible.w * 0.5, visible.y + visible.h * 0.5],
            delta: [0., 100_000.],
        },
        &mut Clip::default(),
    )?;
    ui.frame(&mut text)?;
    assert!(ui.scroll_position(scroll).expect("notes scroll position")[1] > 0.);
    let message = &ui.nodes[ui.find("ara-dialog-1-message").expect("message")];
    let clip = ui.nodes[scroll].rect.intersection(ui.nodes[scroll].clip);
    assert!(
        message.rect.y + message.rect.h <= clip.y + clip.h + 0.5
            && message.rect.y + message.rect.h >= clip.y,
        "the final note line is reachable by real wheel input"
    );
    assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
    Ok(())
}
