//! Offscreen-only toolkit host used by the native input acceptance journey. It
//! mounts real shared controls in the editor's ordinary Ui and InputState.
use ara_ui::{
    Ui,
    dialogs::{DialogFuture, DialogOptions, Dialogs},
    input::{InputEvent, InputState},
    native_input::Router,
    select::{Key, Select, SelectOption},
    source::{escape, stable_id},
};
use ara_ui_text::Clipboard;
use serde_json::{Value, json};

pub(crate) const CSS: &str = "\n.kit-proof { position:absolute;left:0px;top:0px;width:100%;height:100%;align-items:center;justify-content:center;background:var(--overlay); }\n.kit-proof-body { padding:16px;gap:12px;background:var(--popover);border-width:1px;border-color:var(--border);border-radius:8px; }\n.kit-proof-text { white-space:normal;font-size:12px;color:var(--foreground); }\n";
const NOTE: &str = "This is a native toolkit proof. Long notes wrap inside the dialog and remain readable without reaching the screen edge. العربية · 日本語 · 🙂. The editor behind this dialog must retain its content and must not receive keyboard or pointer actions. ";

/// Semantic identity of an actually mounted shared dialog in the acceptance host.
#[derive(Clone, Copy)]
pub(crate) enum DialogKind {
    /// The naming dialog containing a native text field.
    Prompt,
    /// The informational dialog containing the long recovery note.
    Note,
}

pub(crate) struct Host {
    text: String,
    select: Select<usize>,
    selected: usize,
    dialogs: Dialogs,
    prompt: Option<DialogFuture<Option<String>>>,
    alert: Option<DialogFuture<()>>,
    prompt_result: Option<Option<String>>,
    scope: bool,
    markup_dirty: bool,
}

impl Host {
    pub fn new() -> Self {
        let options = (0..100).map(|i| SelectOption::new(i, format!("Choice {i:03} — Éclairage / العربية / 日本語 — a localized option label that must wrap inside the shared maximum width"))).collect();
        Self {
            text: String::new(),
            select: Select::new(
                "kit-long-select",
                "Hundred localized choices",
                options,
                Some(0),
            )
            .with_control_id("kit-select"),
            selected: 0,
            dialogs: Dialogs::new("kit-dialog"),
            prompt: None,
            alert: None,
            prompt_result: None,
            scope: false,
            markup_dirty: false,
        }
    }

    pub fn markup(&self, viewport: [f32; 2]) -> String {
        let width = 620_f32.min((viewport[0] - 32.).max(1.));
        let mut out = format!(
            "<modal id='kit-host' class='kit-proof' label='Native toolkit acceptance'><panel id='kit-host-body' class='kit-proof-body' style='width:{width}px;max-height:{}px'><text id='kit-title' class='kit-proof-text' text='Native toolkit · Unicode, IME, clipboard, Select and modal scopes' /><field id='kit-text' label='Unicode test text' text='{}' action='kit.proof.text' style='min-width:0px;min-height:28px' /><text id='kit-text-help' class='kit-proof-text' text='The verification clipboard is isolated from your desktop clipboard.' />{}<button id='kit-open-note' text='Open long note' action='kit.proof.note' /><button id='kit-open-prompt' text='Open prompt · F2 stacks a second dialog' action='kit.proof.prompt' /></panel></modal>",
            (viewport[1] - 32.).max(1.),
            escape(&self.text, b'\''),
            self.select.control_markup()
        );
        out.push_str(&self.select.popover_markup());
        out.push_str(&self.dialogs.markup(viewport));
        out
    }

    pub fn sync(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<()> {
        if !self.scope && ui.find("kit-host").is_some() {
            input.push(ui, stable_id("kit-host"), Some(stable_id("kit-text")), true)?;
            self.scope = true;
        }
        self.select.sync(ui, input)?;
        self.dialogs.sync(ui, input)
    }

    pub fn event(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        event: InputEvent,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<()> {
        if self.dialogs.is_open() {
            self.dialogs.event(ui, input, event, clipboard)?;
        } else if let Some(event) = self.select.event(ui, input, event, clipboard)? {
            input.event(ui, event, clipboard)?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "acceptance host borrows the production owners without copying them"
    )]
    pub fn key(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        native: &mut Router,
        scan: i32,
        pressed: bool,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<bool> {
        if !pressed {
            native.key(ui, input, scan, false, false, clipboard)?;
            return Ok(true);
        }
        if scan == 59 && self.dialogs.is_open() && self.alert.is_none() {
            self.open_note();
            // Key handlers do not enqueue a widget action. Publish this model
            // change through the same dispatch/reconcile boundary as a click.
            self.markup_dirty = true;
            return Ok(true);
        }
        if self.dialogs.is_open() && matches!(scan, 40 | 88 | 41 | 43) {
            let event = match scan {
                41 => InputEvent::Cancel,
                43 => InputEvent::Tab {
                    reverse: native.modifiers().shift,
                },
                _ => InputEvent::Activate,
            };
            self.dialogs.event(ui, input, event, clipboard)?;
            return Ok(true);
        }
        if self.select.is_open() {
            let key = match scan {
                40 | 88 => Some(Key::Enter),
                41 => Some(Key::Escape),
                81 => Some(Key::Down),
                82 => Some(Key::Up),
                74 => Some(Key::Home),
                77 => Some(Key::End),
                _ => None,
            };
            if let Some(key) = key {
                self.select.key(ui, input, key)?;
                return Ok(true);
            }
            if scan == 43 {
                self.select.event(
                    ui,
                    input,
                    InputEvent::Tab {
                        reverse: native.modifiers().shift,
                    },
                    clipboard,
                )?;
                return Ok(true);
            }
        }
        native.key(ui, input, scan, true, false, clipboard)?;
        Ok(true)
    }

    pub fn dispatch(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<bool> {
        let mut changed = self.select.dispatch(ui, input)?;
        changed |= std::mem::take(&mut self.markup_dirty);
        self.dialogs.dispatch(ui, input)?;
        if let Some(selected) = self.select.take_changed() {
            self.selected = selected;
            changed = true;
        }
        let actions: Vec<_> = ui.drain_actions().collect();
        for action in actions {
            match action.name.as_str() {
                "kit.proof.text" => {
                    if let Some(text) = action.value.as_str() {
                        self.text = text.into();
                        changed = true;
                    }
                }
                "kit.proof.prompt" if self.prompt.is_none() => {
                    let mut options = DialogOptions::new("Unicode prompt", NOTE.repeat(3));
                    options.accept_label = "Save prompt".into();
                    self.prompt = Some(self.dialogs.prompt(options, ""));
                    self.prompt_result = None;
                    changed = true;
                }
                "kit.proof.note" if self.alert.is_none() => {
                    self.open_note();
                    changed = true;
                }
                "kit.proof.prompt" | "kit.proof.note" => {}
                _ => {
                    return Err(format!(
                        "native toolkit proof received unexpected action: {}",
                        action.name
                    )
                    .into());
                }
            }
        }
        if let Some(value) = self.prompt.as_mut().and_then(DialogFuture::try_take) {
            self.prompt_result = Some(value);
            self.prompt = None;
            changed = true;
        }
        if self
            .alert
            .as_mut()
            .and_then(DialogFuture::try_take)
            .is_some()
        {
            self.alert = None;
            changed = true;
        }
        Ok(changed)
    }

    fn open_note(&mut self) {
        self.alert = Some(self.dialogs.alert(DialogOptions::new(
            "A second native dialog",
            NOTE.repeat(16),
        )));
    }

    pub fn close(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<()> {
        self.dialogs.suspend(ui, input);
        self.select.key(ui, input, Key::Escape)?;
        if self.scope {
            if let Some(index) = ui.find("kit-host") {
                ui.set_visible(index, false)?;
            }
            input.remove_scope(ui, stable_id("kit-host"));
            self.scope = false;
        }
        Ok(())
    }

    /// Resolve the mounted surface by its real control structure, never a request counter.
    pub fn dialog_id<'a>(&self, ui: &'a Ui, kind: DialogKind) -> Option<&'a str> {
        ui.nodes
            .first()?
            .element
            .children
            .iter()
            .rev()
            .find_map(|index| {
                let node = &ui.nodes[*index];
                if node.element.tag != "modal"
                    || !node.element.id.starts_with("kit-dialog-")
                    || !ui.shown(*index)
                {
                    return None;
                }
                let has_field = ui.find(&format!("{}-field", node.element.id)).is_some();
                (has_field == matches!(kind, DialogKind::Prompt))
                    .then_some(node.element.id.as_str())
            })
    }
    pub fn snapshot(&self, ui: &Ui) -> Value {
        json!({"text":self.text,"selected":self.selected,"selectOpen":self.select.is_open(),"selectQuery":self.select.query(),"selectActive":self.select.active().map(|option|option.value),"dialogCount":self.dialogs.len(),"promptResult":self.prompt_result,
            "promptId":self.dialog_id(ui,DialogKind::Prompt),"noteId":self.dialog_id(ui,DialogKind::Note),"markupDirty":self.markup_dirty})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ui::theme::EDITOR_CSS;
    use ara_ui_text::{EditEvent, TextSystem};

    #[derive(Default)]
    struct Clip;
    impl Clipboard for Clip {
        fn read(&mut self) -> ara_ui_text::Result<String> {
            Ok(String::new())
        }
        fn write(&mut self, _text: &str) -> ara_ui_text::Result<()> {
            Ok(())
        }
    }
    fn mount(
        host: &mut Host,
        ui: &mut Ui,
        input: &mut InputState,
        text: &mut TextSystem,
    ) -> ara_ui::Result<()> {
        ui.reconcile(&format!(
            "<ui id='root'><button id='outside' text='Outside' />{}</ui>",
            host.markup(ui.size)
        ))?;
        ui.frame(text)?;
        host.sync(ui, input)?;
        ui.frame(text)?;
        Ok(())
    }
    #[test]
    fn toolkit_host_removal_restores_unscoped_editor_focus_and_keeps_an_unfocused_editor_unfocused()
    -> ara_ui::Result<()> {
        let base = "<button id='outside' text='First editor button' /><field id='editor-draft' text='Keep this draft' />";
        let mut text = TextSystem::bundled()?;
        for focused in [true, false] {
            let mut host = Host::new();
            let mut ui = Ui::editor(
                &format!("<ui id='root'>{base}</ui>"),
                &format!(
                    "{EDITOR_CSS}{CSS}{}{}",
                    ara_ui::dialogs::CSS,
                    ara_ui::select::CSS
                ),
                [1440., 900.],
            )?;
            ui.frame(&mut text)?;
            let mut input = InputState::default();
            if focused {
                input.focus(&mut ui, stable_id("editor-draft"))?;
            }
            let expected = input.focused();
            host.open_note();
            ui.reconcile(&format!(
                "<ui id='root'>{base}{}</ui>",
                host.markup(ui.size)
            ))?;
            ui.frame(&mut text)?;
            host.sync(&mut ui, &mut input)?;
            ui.frame(&mut text)?;
            assert!(host.dialog_id(&ui, DialogKind::Note).is_some());
            assert_ne!(input.focused(), expected);
            host.close(&mut ui, &mut input)?;
            ui.reconcile(&format!("<ui id='root'>{base}</ui>"))?;
            ui.frame(&mut text)?;
            assert!(ui.find("kit-host").is_none());
            assert_eq!(
                input.focused(),
                expected,
                "Removing the proof host must preserve the editor's exact original focus"
            );
            let field = ui.widgets.get(&stable_id("editor-draft"));
            assert!(
                matches!(field, Some(ara_ui::widgets::Widget::Input(field)) if field.value() == "Keep this draft")
            );
        }
        Ok(())
    }
    #[test]
    fn keyboard_stacked_dialog_requests_reconcile_before_escape_and_restores_prompt_draft()
    -> ara_ui::Result<()> {
        let mut host = Host::new();
        let mut ui = Ui::editor(
            "<ui id='root'><button id='outside' text='Outside' /></ui>",
            &format!(
                "{EDITOR_CSS}{CSS}{}{}",
                ara_ui::dialogs::CSS,
                ara_ui::select::CSS
            ),
            [1440., 900.],
        )?;
        let mut text = TextSystem::bundled()?;
        ui.frame(&mut text)?;
        let mut input = InputState::default();
        input.push(
            &mut ui,
            stable_id("root"),
            Some(stable_id("outside")),
            false,
        )?;
        let mut native = Router::default();
        let mut clipboard = Clip;
        // An earlier alert advances the controller's request sequence, so the
        // prompt and stacked note cannot rely on fixed numeric identifiers.
        host.open_note();
        mount(&mut host, &mut ui, &mut input, &mut text)?;
        host.key(&mut ui, &mut input, &mut native, 40, true, &mut clipboard)?;
        assert!(host.dispatch(&mut ui, &mut input)?);
        mount(&mut host, &mut ui, &mut input, &mut text)?;
        let point = ui.nodes[ui.find("kit-open-prompt").ok_or("Prompt button missing")?].rect;
        let point = [point.x + point.w * 0.5, point.y + point.h * 0.5];
        host.event(
            &mut ui,
            &mut input,
            InputEvent::PointerDown(point),
            &mut clipboard,
        )?;
        host.event(
            &mut ui,
            &mut input,
            InputEvent::PointerUp(point),
            &mut clipboard,
        )?;
        assert!(host.dispatch(&mut ui, &mut input)?);
        mount(&mut host, &mut ui, &mut input, &mut text)?;
        let prompt = host
            .dialog_id(&ui, DialogKind::Prompt)
            .ok_or("Prompt was not mounted")?
            .to_owned();
        assert_ne!(prompt, "kit-dialog-1");
        host.event(
            &mut ui,
            &mut input,
            InputEvent::Edit(EditEvent::Insert("Preserved ✓".into())),
            &mut clipboard,
        )?;
        host.key(&mut ui, &mut input, &mut native, 43, true, &mut clipboard)?;
        let restore = input.focused();
        host.key(&mut ui, &mut input, &mut native, 59, true, &mut clipboard)?;
        assert_eq!(host.snapshot(&ui)["dialogCount"], 2);
        assert!(
            host.dialog_id(&ui, DialogKind::Note).is_none(),
            "The queued request has not mounted yet"
        );
        assert!(
            host.dispatch(&mut ui, &mut input)?,
            "F2 must request the host's normal markup reconciliation"
        );
        mount(&mut host, &mut ui, &mut input, &mut text)?;
        let note = host
            .dialog_id(&ui, DialogKind::Note)
            .ok_or("Stacked note was not mounted")?
            .to_owned();
        assert_ne!(note, prompt);
        assert_eq!(input.focused(), Some(stable_id(&format!("{note}-accept"))));
        assert!(
            !host.dispatch(&mut ui, &mut input)?,
            "The change flag is consumed once"
        );
        host.key(&mut ui, &mut input, &mut native, 41, true, &mut clipboard)?;
        assert!(host.dispatch(&mut ui, &mut input)?);
        mount(&mut host, &mut ui, &mut input, &mut text)?;
        assert_eq!(host.snapshot(&ui)["dialogCount"], 1);
        assert!(host.dialog_id(&ui, DialogKind::Note).is_none());
        assert_eq!(input.focused(), restore);
        input.focus(&mut ui, stable_id(&format!("{prompt}-field")))?;
        host.key(&mut ui, &mut input, &mut native, 40, true, &mut clipboard)?;
        assert!(host.dispatch(&mut ui, &mut input)?);
        mount(&mut host, &mut ui, &mut input, &mut text)?;
        assert_eq!(host.snapshot(&ui)["promptResult"], "Preserved ✓");
        assert_eq!(input.focused(), Some(stable_id("kit-open-prompt")));
        Ok(())
    }
}
