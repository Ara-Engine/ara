//! Retained kit selects lower accepted typed values to the Hub model.
use crate::{
    model::Intent,
    view::{Choice, Page},
};
use ara_ui::{
    Ui,
    input::{InputEvent, InputState},
    select::{Key, Select},
    source::stable_id,
};
use ara_ui_text::Clipboard;
use std::collections::{BTreeMap, BTreeSet};

struct Entry {
    descriptor: Choice,
    select: Select<Intent>,
}

#[derive(Default)]
pub(super) struct Selects {
    entries: BTreeMap<String, Entry>,
    dirty: bool,
}

impl Selects {
    /// Replace closed fixture markup with retained controls and root-level popups.
    pub fn prepare(
        &mut self,
        page: &mut Page,
        ui: &mut Ui,
        input: &mut InputState,
    ) -> ara_ui::Result<()> {
        let controls: BTreeSet<_> = page
            .choices
            .iter()
            .map(|choice| choice.control.clone())
            .collect();
        for (id, entry) in &mut self.entries {
            if !controls.contains(id) {
                entry.select.key(ui, input, Key::Escape)?;
            }
        }
        self.entries.retain(|id, _| controls.contains(id));
        let mut popups = String::new();
        for choice in std::mem::take(&mut page.choices) {
            let entry = self
                .entries
                .entry(choice.control.clone())
                .or_insert_with(|| Entry {
                    select: choice.select(),
                    descriptor: choice.clone(),
                });
            if entry.descriptor.options != choice.options {
                entry.select.set_options(choice.kit_options());
            }
            if entry.select.selected() != choice.selected.as_ref() {
                entry.select.set_selected(choice.selected.clone());
            }
            page.markup = page
                .markup
                .replacen(&choice.markup, &entry.select.control_markup(), 1);
            popups.push_str(&entry.select.popover_markup());
            entry.descriptor = choice;
        }
        if let Some(end) = page.markup.rfind("</ui>") {
            page.markup.insert_str(end, &popups);
        }
        self.dirty = false;
        Ok(())
    }

    /// Keep popup scope and scroll placement aligned with reconciled nodes.
    pub fn sync(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<()> {
        for entry in self.entries.values_mut() {
            entry.select.sync(ui, input)?;
        }
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.entries.values().any(|entry| entry.select.is_open())
    }

    /// Close child popup scopes before their parent modal is replaced.
    pub fn dismiss(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<()> {
        for entry in self.entries.values_mut() {
            if entry.select.is_open() {
                entry.select.key(ui, input, Key::Escape)?;
                self.dirty = true;
            }
        }
        Ok(())
    }

    fn target(&mut self, input: &InputState) -> Option<&mut Entry> {
        let open = self
            .entries
            .iter()
            .find(|(_, entry)| entry.select.is_open())
            .map(|(id, _)| id.clone());
        if let Some(id) = open {
            return self.entries.get_mut(&id);
        }
        let focused = input.focused()?;
        self.entries
            .values_mut()
            .find(|entry| stable_id(entry.select.control_id()) == focused)
    }

    /// Return only events the kit has not consumed; never route the original twice.
    pub fn event(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        event: InputEvent,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<Option<InputEvent>> {
        let state_change = !matches!(
            &event,
            InputEvent::PointerMove(_) | InputEvent::Wheel { .. }
        );
        let Some(entry) = self.target(input) else {
            return Ok(Some(event));
        };
        let event = entry.select.event(ui, input, event, clipboard)?;
        self.dirty |= state_change;
        Ok(event)
    }

    pub fn key(&mut self, ui: &mut Ui, input: &mut InputState, key: Key) -> ara_ui::Result<bool> {
        let Some(entry) = self.target(input) else {
            return Ok(false);
        };
        let changed = entry.select.key(ui, input, key)?;
        self.dirty |= changed;
        Ok(changed)
    }

    /// Consume owned kit actions before the Hub drains its action bindings.
    pub fn actions(&mut self, ui: &mut Ui, input: &mut InputState) -> ara_ui::Result<Vec<Intent>> {
        let mut intents = Vec::new();
        for entry in self.entries.values_mut() {
            self.dirty |= entry.select.dispatch(ui, input)?;
            if let Some(value) = entry.select.take_changed()
                && entry.descriptor.selected.as_ref() != Some(&value)
            {
                intents.push(value);
            }
        }
        Ok(intents)
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn get(&self, control: &str) -> Option<&Select<Intent>> {
        self.entries.get(control).map(|entry| &entry.select)
    }
}
