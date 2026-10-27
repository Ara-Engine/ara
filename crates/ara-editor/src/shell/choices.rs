//! Shared native searchable selects for Inspector, Settings and importer fields.
use super::*;
use ara_ui::{
    input::InputEvent,
    select::{Key, Select as NativeSelect, SelectOption},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Destination {
    Setting(String),
    Import { doc: String, key: String },
    Field(String),
    Append { field: String, pointer: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Select {
    pub destination: Destination,
    pub options: Vec<(Value, String)>,
    pub current: Value,
}

struct Control {
    destination: Destination,
    options: Vec<(Value, String)>,
    select: NativeSelect<Value>,
}

/// Retained typed selects keyed by surface and authored control identity.
#[derive(Default)]
pub(crate) struct Controls {
    controls: BTreeMap<(Option<String>, String), Control>,
}

impl Shell {
    pub(super) fn choice_button(
        &mut self,
        id: &str,
        label: &str,
        select: Select,
        out: &mut String,
    ) {
        let key = (self.current.clone(), id.to_owned());
        let entry = self
            .choice_controls
            .controls
            .entry(key)
            .or_insert_with(|| Control {
                destination: select.destination.clone(),
                options: select.options.clone(),
                select: NativeSelect::new(
                    format!("choice-{id}"),
                    label,
                    select
                        .options
                        .iter()
                        .map(|(value, label)| SelectOption::new(value.clone(), label))
                        .collect(),
                    Some(select.current.clone()),
                )
                .with_control_id(id),
            });
        if entry.destination != select.destination {
            entry.destination = select.destination;
        }
        if entry.options != select.options {
            entry.select.set_options(
                select
                    .options
                    .iter()
                    .map(|(value, label)| SelectOption::new(value.clone(), label))
                    .collect(),
            );
            entry.options = select.options;
        }
        entry.select.set_selected(Some(select.current));
        out.push_str(&entry.select.control_markup());
    }

    pub(super) fn choose_value(
        &mut self,
        editor: &mut Editor,
        target: &Destination,
        value: Value,
    ) -> ara_ui::Result<()> {
        match target {
            Destination::Setting(key) => {
                editor.action("editor.settings.set", json!({"key":key,"value":value}))?;
            }
            Destination::Import { doc, key } => {
                self.drafts
                    .insert(format!("@import|{doc}|{key}"), value.to_string());
                self.commit_import_fields(editor)?;
            }
            Destination::Field(path) => {
                editor.apply(
                    self.field_commands(editor, &editor.state.selection, path, value)?,
                    &format!("Set {path}"),
                    None,
                )?;
            }
            Destination::Append { field, pointer } => {
                let field = self
                    .fields
                    .get(field)
                    .ok_or("The collection field is no longer visible")?;
                let commands = collection_adapter::commands(
                    editor,
                    &editor.state.selection,
                    field,
                    &field.value,
                    pointer,
                    crate::commands::nested::Operation::Append,
                    value,
                    None,
                )?;
                editor.apply(commands, "Add collection item", None)?;
            }
        }
        self.menu = None;
        self.rebuild(editor)
    }

    /// Append every open select for this surface under the UI root.
    pub(super) fn choice_overlays(&self, out: &mut String) {
        for ((surface, _), control) in &self.choice_controls.controls {
            if surface == &self.current {
                out.push_str(&control.select.popover_markup());
            }
        }
    }

    /// Restore retained popup scopes after root markup has been laid out.
    pub(super) fn sync_choice_controls(&mut self) -> ara_ui::Result<()> {
        for ((surface, _), control) in &mut self.choice_controls.controls {
            if surface == &self.current {
                control.select.sync(&mut self.ui, &mut self.input)?;
            }
        }
        Ok(())
    }

    /// Route real input through the shared controller without copying events.
    pub(super) fn choice_event(
        &mut self,
        event: InputEvent,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<Option<InputEvent>> {
        let mut event = Some(event);
        for ((surface, _), control) in &mut self.choice_controls.controls {
            if surface != &self.current {
                continue;
            }
            let Some(current) = event.take() else {
                break;
            };
            event = control
                .select
                .event(&mut self.ui, &mut self.input, current, clipboard)?;
        }
        Ok(event)
    }

    /// Consume only select-owned actions and lower accepted typed values through
    /// the same settings/import/field commands used by agents and normal edits.
    pub(super) fn dispatch_choices(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changes = Vec::new();
        let mut changed = false;
        for ((surface, _), control) in &mut self.choice_controls.controls {
            if surface != &self.current {
                continue;
            }
            changed |= control.select.dispatch(&mut self.ui, &mut self.input)?;
            if let Some(value) = control.select.take_changed() {
                changes.push((control.destination.clone(), value));
            }
        }
        for (destination, value) in changes {
            self.choose_value(editor, &destination, value)?;
        }
        Ok(changed)
    }

    /// Whether a select popup currently owns this surface's input.
    pub(crate) fn choice_open(&self) -> bool {
        self.choice_controls
            .controls
            .iter()
            .any(|((surface, _), control)| surface == &self.current && control.select.is_open())
    }

    /// Translate native keys once for every select consumer. Ordinary committed
    /// text still arrives through choice_event and the retained native field.
    pub(super) fn choice_key(
        &mut self,
        editor: &mut Editor,
        key: i32,
        pressed: bool,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<bool> {
        use ara_ui_text::EditEvent;
        if !pressed {
            return Ok(false);
        }
        let focused = self.input.focused();
        let Some((_, control)) =
            self.choice_controls
                .controls
                .iter_mut()
                .find(|((surface, _), control)| {
                    surface == &self.current
                        && (control.select.is_open()
                            || focused
                                == Some(ara_ui::source::stable_id(control.select.control_id())))
                })
        else {
            return Ok(false);
        };
        let open = control.select.is_open();
        let [shift, ctrl, alt] = self.modifiers;
        let navigation = match key {
            40 | 88 => Some(Key::Enter),
            81 => Some(Key::Down),
            82 => Some(Key::Up),
            74 => Some(Key::Home),
            77 => Some(Key::End),
            41 => Some(Key::Escape),
            _ => None,
        };
        let handled = if let Some(key) = navigation {
            control.select.key(&mut self.ui, &mut self.input, key)?
        } else if open {
            let event = match key {
                43 => Some(InputEvent::Tab { reverse: shift }),
                4 if ctrl => Some(InputEvent::Edit(EditEvent::SelectAll)),
                6 if ctrl => Some(InputEvent::Edit(EditEvent::Copy)),
                27 if ctrl => Some(InputEvent::Edit(EditEvent::Cut)),
                25 if ctrl => Some(InputEvent::Edit(EditEvent::Paste)),
                29 if ctrl => Some(InputEvent::Edit(if shift {
                    EditEvent::Redo
                } else {
                    EditEvent::Undo
                })),
                28 if ctrl => Some(InputEvent::Edit(EditEvent::Redo)),
                42 => Some(InputEvent::Edit(EditEvent::Backspace)),
                76 => Some(InputEvent::Edit(EditEvent::Delete)),
                79 | 80 => Some(InputEvent::Edit(EditEvent::Move {
                    forward: key == 79,
                    select: shift,
                })),
                _ => None,
            };
            if let Some(event) = event {
                control
                    .select
                    .event(&mut self.ui, &mut self.input, event, clipboard)?;
                true
            } else {
                ctrl || alt || (58..=69).contains(&key)
            }
        } else {
            false
        };
        if handled {
            self.dispatch_choices(editor)?;
            self.rebuild(editor)?;
        }
        Ok(handled)
    }
}
