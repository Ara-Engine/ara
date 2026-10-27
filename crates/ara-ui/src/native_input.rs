//! Coordinate-free native keyboard, Unicode, IME and focus-loss semantics.
//! Hosts choose one text ingress: never feed both SDL commits and WM_CHAR into a
//! field. Physical keys produce editing/navigation only, never printable text.
use crate::{
    Result, Ui,
    input::{Direction, InputEvent, InputState},
    widgets::Widget,
};
use ara_ui_text::{Clipboard, EditEvent};

/// The platform's primary shortcut modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryModifier {
    /// Control on Windows and Linux.
    Control,
    /// Command on macOS.
    Command,
}
impl Default for PrimaryModifier {
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Self::Command
        } else {
            Self::Control
        }
    }
}

/// The sole committed-text/preedit ingress selected by a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSource {
    /// SDL_TEXTINPUT and SDL_TEXTEDITING.
    Sdl,
    /// A host's native text adapter, such as Windows WM_CHAR/IME.
    Native,
}

/// Current native modifiers, including independent left/right key tracking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Either Shift key is down.
    pub shift: bool,
    /// Either Control key is down.
    pub control: bool,
    /// Either Alt/Option key is down.
    pub alt: bool,
    /// Either Command/Windows key is down.
    pub command: bool,
}

/// Per-window native semantic state; it owns no coordinates, application model or
/// OS handle. Clipboard access is supplied by the host's existing native adapter.
pub struct Router {
    primary: PrimaryModifier,
    text_source: TextSource,
    modifiers: [bool; 8],
    composition: Option<u64>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new(PrimaryModifier::default(), TextSource::Sdl)
    }
}

impl Router {
    /// Explicitly choose the shortcut modifier and exactly one text ingress.
    pub fn new(primary: PrimaryModifier, text_source: TextSource) -> Self {
        Self {
            primary,
            text_source,
            modifiers: [false; 8],
            composition: None,
        }
    }

    /// Current modifier state, without conflating Control and Command.
    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            control: self.modifiers[0] || self.modifiers[4],
            shift: self.modifiers[1] || self.modifiers[5],
            alt: self.modifiers[2] || self.modifiers[6],
            command: self.modifiers[3] || self.modifiers[7],
        }
    }

    /// Whether a field currently owns uncommitted IME text.
    pub fn composing(&self) -> bool {
        self.composition.is_some()
    }

    /// Whether the current focused control can receive text in the active scope.
    pub fn text_field(&self, ui: &Ui, input: &InputState) -> Option<usize> {
        input.focused().and_then(|id| ui.by_id(id)).filter(|i| {
            ui.shown(*i)
                && !ui.nodes[*i].state.disabled
                && ui.accepts_input(*i)
                && input.in_scope(ui, *i)
                && matches!(
                    ui.widgets
                        .get(&crate::source::stable_id(&ui.nodes[*i].element.id)),
                    Some(Widget::Input(_))
                )
        })
    }

    /// Handle native editing/navigation keys. True prevents the same physical key
    /// from leaking into gameplay; printable characters arrive only through commit.
    pub fn key(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        scancode: i32,
        pressed: bool,
        repeat: bool,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        self.sync_focus(ui, input, clipboard)?;
        if (224..=231).contains(&scancode) {
            self.modifiers[(scancode - 224) as usize] = pressed;
        }
        let field = self.text_field(ui, input);
        if !pressed || (224..=231).contains(&scancode) {
            return Ok(field.is_some());
        }
        let modifiers = self.modifiers();
        let primary = !modifiers.alt
            && match self.primary {
                PrimaryModifier::Control => modifiers.control,
                PrimaryModifier::Command => modifiers.command,
            };
        if scancode == 41 {
            if self.composition.is_some() {
                self.cancel_composition(ui, clipboard)?;
                return Ok(true);
            }
            return Ok(false);
        }
        let event = match scancode {
            43 if input.focused().is_some() => Some(InputEvent::Tab {
                reverse: modifiers.shift,
            }),
            40 | 88 if !repeat && field.is_none() && input.focused().is_some() => {
                Some(InputEvent::Activate)
            }
            44 if !repeat && field.is_none() && input.focused().is_some() => {
                Some(InputEvent::Activate)
            }
            4 if primary && field.is_some() => Some(InputEvent::Edit(EditEvent::SelectAll)),
            6 if primary && field.is_some() => Some(InputEvent::Edit(EditEvent::Copy)),
            27 if primary && field.is_some() => Some(InputEvent::Edit(EditEvent::Cut)),
            25 if primary && field.is_some() => Some(InputEvent::Edit(EditEvent::Paste)),
            29 if primary && field.is_some() => Some(InputEvent::Edit(if modifiers.shift {
                EditEvent::Redo
            } else {
                EditEvent::Undo
            })),
            28 if primary && field.is_some() => Some(InputEvent::Edit(EditEvent::Redo)),
            42 if field.is_some() => Some(InputEvent::Edit(EditEvent::Backspace)),
            76 if field.is_some() => Some(InputEvent::Edit(EditEvent::Delete)),
            79 | 80 if field.is_some() => Some(InputEvent::Edit(EditEvent::Move {
                forward: scancode == 79,
                select: modifiers.shift,
            })),
            74 | 77 if field.is_some() => {
                let widget = field.and_then(|i| {
                    ui.widgets
                        .get(&crate::source::stable_id(&ui.nodes[i].element.id))
                });
                if let Some(Widget::Input(text)) = widget {
                    let caret = if scancode == 74 {
                        0
                    } else {
                        text.value().len()
                    };
                    Some(InputEvent::Edit(EditEvent::Select {
                        anchor: if modifiers.shift {
                            text.anchor()
                        } else {
                            caret
                        },
                        caret,
                    }))
                } else {
                    None
                }
            }
            79..=82 if input.focused().is_some() => Some(InputEvent::Navigate(match scancode {
                79 => Direction::Right,
                80 => Direction::Left,
                81 => Direction::Down,
                _ => Direction::Up,
            })),
            _ => None,
        };
        if let Some(event) = event {
            self.cancel_composition(ui, clipboard)?;
            input.event(ui, event, clipboard)?;
            Ok(true)
        } else {
            Ok(field.is_some())
        }
    }

    /// Commit native UTF-8 once. An unselected ingress is ignored, including an
    /// SDL echo when a Windows host has selected its native WM_CHAR text path.
    pub fn commit(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        source: TextSource,
        text: &str,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        if source != self.text_source {
            return Ok(false);
        }
        self.sync_focus(ui, input, clipboard)?;
        if self.text_field(ui, input).is_none() {
            return Ok(false);
        }
        input.event(
            ui,
            InputEvent::Edit(EditEvent::ImeCommit(text.into())),
            clipboard,
        )?;
        self.composition = None;
        Ok(true)
    }

    /// Update uncommitted IME text. SDL's character offsets are clamped and
    /// converted to valid UTF-8 byte boundaries before reaching the retained field.
    pub fn preedit(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        source: TextSource,
        text: &str,
        start: i32,
        length: i32,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        if source != self.text_source {
            return Ok(false);
        }
        self.sync_focus(ui, input, clipboard)?;
        if self.text_field(ui, input).is_none() {
            return Ok(false);
        }
        if text.is_empty() {
            self.cancel_composition(ui, clipboard)?;
            return Ok(true);
        }
        if text.chars().any(char::is_control) {
            self.cancel_composition(ui, clipboard)?;
            return Ok(false);
        }
        if self.composition.is_none() {
            input.event(ui, InputEvent::Edit(EditEvent::ImeStart), clipboard)?;
            self.composition = input.focused();
        }
        let begin = start.max(0) as usize;
        let end = begin.saturating_add(length.max(0) as usize);
        let offset = |count| {
            text.char_indices()
                .nth(count)
                .map_or(text.len(), |(byte, _)| byte)
        };
        input.event(
            ui,
            InputEvent::Edit(EditEvent::ImePreedit {
                text: text.into(),
                selection: offset(begin)..offset(end),
            }),
            clipboard,
        )?;
        Ok(true)
    }

    /// Release modifiers, IME preview and pointer gestures without closing any
    /// modal or screen. The same seam handles native capture loss.
    pub fn focus_lost(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        self.modifiers = [false; 8];
        self.cancel_composition(ui, clipboard)?;
        input.cancel_interaction(ui);
        Ok(())
    }

    /// Cancel a preedit whose field lost focus or disappeared after a pointer
    /// event, screen change or host reconcile. Committed values remain intact.
    pub fn sync_focus(
        &mut self,
        ui: &mut Ui,
        input: &InputState,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        if self.composition.is_some()
            && (self.composition != input.focused() || self.text_field(ui, input).is_none())
        {
            self.cancel_composition(ui, clipboard)?;
        }
        Ok(())
    }

    fn cancel_composition(&mut self, ui: &mut Ui, clipboard: &mut dyn Clipboard) -> Result<()> {
        if let Some(index) = self.composition.take().and_then(|id| ui.by_id(id))
            && matches!(
                ui.widgets
                    .get(&crate::source::stable_id(&ui.nodes[index].element.id)),
                Some(Widget::Input(_))
            )
        {
            ui.edit(index, EditEvent::ImeCancel, clipboard)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
