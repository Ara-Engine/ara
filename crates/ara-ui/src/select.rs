//! Searchable, typed selects shared by editor, Hub and game UI.
//!
//! Mount control_markup in the form and popover_markup under the UI root, include
//! CSS, then sync after layout. Route events through event and pass any returned
//! event to InputState, then dispatch. A chosen value is an intent
//! from take_changed: the host validates and commits its typed command.
mod view;
use crate::{
    Result, Ui,
    input::{Direction, InputEvent, InputState},
    layers::{LayerKind, LayerStack},
    source::stable_id,
};
pub use view::CSS;

/// A typed value and its user-facing label, independent of row position.
#[derive(Clone, Debug)]
pub struct SelectOption<T> {
    /// Value returned to the host when this option is chosen.
    pub value: T,
    /// Localized label; long labels wrap within the shared overlay width limit.
    pub label: String,
    /// Additional accessible/hover explanation.
    pub description: Option<String>,
    /// A reason this option cannot be chosen; displayed and announced when present.
    pub disabled: Option<String>,
}

impl<T> SelectOption<T> {
    /// An enabled option with a localized label.
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            description: None,
            disabled: None,
        }
    }
}

/// Physical-key intent translated by the native host, including type-to-filter.
#[derive(Clone, Debug)]
pub enum Key {
    /// Previous enabled matching option.
    Up,
    /// Next enabled matching option, or open the closed control.
    Down,
    /// First enabled matching option.
    Home,
    /// Last enabled matching option.
    End,
    /// Open, or choose the active option.
    Enter,
    /// Dismiss without changing the value.
    Escape,
    /// Open and filter using native committed text.
    Text(String),
}

/// Retained state for one searchable select, with a typed host-owned value.
pub struct Select<T> {
    id: String,
    control: String,
    label: String,
    options: Vec<SelectOption<T>>,
    selected: Option<T>,
    changed: Option<T>,
    open: bool,
    query: String,
    active: Option<usize>,
    generation: u64,
    layers: LayerStack,
}

impl<T: Clone + PartialEq> Select<T> {
    /// Create a control with a unique authored prefix and its current host value.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        options: Vec<SelectOption<T>>,
        selected: Option<T>,
    ) -> Self {
        let id = id.into();
        let mut select = Self {
            control: format!("{id}-control"),
            id,
            label: label.into(),
            options,
            selected,
            changed: None,
            open: false,
            query: String::new(),
            active: None,
            generation: 0,
            layers: LayerStack::default(),
        };
        select.repair_active();
        select
    }

    /// Preserve an existing form control's authored ID while namespacing its popup.
    pub fn with_control_id(mut self, id: impl Into<String>) -> Self {
        self.control = id.into();
        self
    }

    /// Current committed value; filtering and keyboard movement never alter it.
    pub fn selected(&self) -> Option<&T> {
        self.selected.as_ref()
    }

    /// Whether the popup currently owns input.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The current filter text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The enabled option currently addressed by keyboard navigation.
    pub fn active(&self) -> Option<&SelectOption<T>> {
        self.active.and_then(|i| self.options.get(i))
    }

    /// Apply a host notification without emitting a user intent.
    pub fn set_selected(&mut self, value: Option<T>) {
        self.selected = value;
        if !self.open {
            self.repair_active();
        }
    }

    /// Replace options while preserving the active typed value when possible.
    /// Generational row IDs reject clicks queued by an older option projection.
    pub fn set_options(&mut self, options: Vec<SelectOption<T>>) {
        let active = self.active().map(|o| o.value.clone());
        self.options = options;
        self.generation = self.generation.wrapping_add(1);
        self.active = active.and_then(|value| {
            self.options
                .iter()
                .position(|o| o.value == value && o.disabled.is_none())
        });
        self.repair_active();
    }

    /// Take one accepted value to lower through the host's typed command API.
    pub fn take_changed(&mut self) -> Option<T> {
        self.changed.take()
    }

    /// Open without changing the value, optionally with an initial text filter.
    pub fn open(&mut self, query: impl Into<String>) {
        self.open = true;
        self.query = query.into();
        self.active = self
            .selected
            .as_ref()
            .and_then(|value| self.options.iter().position(|o| &o.value == value));
        self.repair_active();
    }

    /// Synchronize the anchored popup's input scope after mounting and layout.
    pub fn sync(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<()> {
        self.layers.sync(ui, input)?;
        if ui
            .find(self.control_id())
            .is_none_or(|index| !ui.shown(index))
        {
            self.close(ui, input)?;
            return Ok(());
        }
        let popup = self.popup_id();
        if self.open
            && self.layers.nodes().next().is_none()
            && let Some(index) = ui.find(&popup)
        {
            let rect = ui.nodes[index].rect;
            self.layers.open(
                ui,
                input,
                stable_id(&popup),
                LayerKind::Popup,
                [rect.x, rect.y],
                Some(stable_id(&self.search_id())),
            )?;
        }
        if let Some(crate::widgets::Widget::Choice { open, .. }) =
            ui.widgets.get_mut(&stable_id(self.control_id()))
        {
            *open = self.open;
        }
        self.reveal_active(ui)
    }

    /// Handle an open popup's real UI input. Return an unconsumed event for the
    /// host to route normally, then call dispatch. Closed type-to-filter also works
    /// when the select's control holds focus.
    pub fn event(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        event: InputEvent,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> Result<Option<InputEvent>> {
        if !self.open {
            match event {
                InputEvent::Edit(
                    ara_ui_text::EditEvent::Insert(text) | ara_ui_text::EditEvent::ImeCommit(text),
                ) if input.focused() == Some(stable_id(self.control_id())) => {
                    self.open(text);
                    return Ok(None);
                }
                event => return Ok(Some(event)),
            }
        }
        match event {
            InputEvent::Cancel => self.close(ui, input)?,
            InputEvent::Navigate(Direction::Down) => {
                self.key(ui, input, Key::Down)?;
            }
            InputEvent::Navigate(Direction::Up) => {
                self.key(ui, input, Key::Up)?;
            }
            InputEvent::Activate => {
                self.key(ui, input, Key::Enter)?;
            }
            InputEvent::Tab { reverse } => {
                self.close(ui, input)?;
                input.event(ui, InputEvent::Tab { reverse }, clipboard)?;
            }
            InputEvent::PointerDown(point) if !self.contains(ui, point) => {
                let own_control = ui
                    .find(self.control_id())
                    .is_some_and(|i| ui.nodes[i].rect.contains(point[0], point[1]));
                self.close(ui, input)?;
                if !own_control {
                    return Ok(Some(InputEvent::PointerDown(point)));
                }
            }
            event => {
                input.event(ui, event, clipboard)?;
                self.dispatch(ui, input)?;
            }
        }
        Ok(None)
    }

    /// Route native keys that are not represented by InputEvent (Home and End),
    /// or route all select keys consistently through one host adapter.
    pub fn key(&mut self, ui: &mut Ui, input: &mut InputState, key: Key) -> Result<bool> {
        if !self.open {
            match key {
                Key::Enter | Key::Down => self.open(""),
                Key::Text(text) => self.open(text),
                _ => return Ok(false),
            }
            return Ok(true);
        }
        match key {
            Key::Escape => self.close(ui, input)?,
            Key::Enter => {
                let focused = input.focused().and_then(|id| {
                    (0..self.options.len()).find(|i| id == stable_id(&self.option_id(*i)))
                });
                if let Some(index) = focused.or(self.active) {
                    self.choose(ui, input, index)?;
                }
            }
            Key::Text(text) => {
                self.query.push_str(&text);
                self.repair_active();
            }
            key => {
                let matching = self.matching();
                let at = self
                    .active
                    .and_then(|i| matching.iter().position(|candidate| *candidate == i));
                let next = match key {
                    Key::Home => matching.first().copied(),
                    Key::End => matching.last().copied(),
                    Key::Up => matching.get(at.map_or(0, |i| i.saturating_sub(1))).copied(),
                    Key::Down => matching
                        .get(at.map_or(0, |i| (i + 1).min(matching.len().saturating_sub(1))))
                        .copied(),
                    _ => None,
                };
                self.active = next;
            }
        }
        self.reveal_active(ui)?;
        Ok(true)
    }

    /// Consume only actions emitted by this control. Unrelated actions remain
    /// queued for the host or another select. True means markup/state changed.
    pub fn dispatch(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<bool> {
        let mut changed = false;
        let actions = std::mem::take(&mut ui.actions);
        for action in actions {
            if action.node == stable_id(self.control_id()) && action.name == "ara.select.toggle" {
                if self.open {
                    self.close(ui, input)?;
                } else {
                    self.open("");
                }
                changed = true;
            } else if self.open
                && action.node == stable_id(&self.search_id())
                && action.name == "ara.select.query"
            {
                if let Some(query) = action.value.as_str() {
                    self.query = query.into();
                    self.repair_active();
                    changed = true;
                }
            } else if action.name == self.choose_action() {
                if self.open
                    && let Some(index) = (0..self.options.len())
                        .find(|i| action.node == stable_id(&self.option_id(*i)))
                {
                    self.choose(ui, input, index)?;
                    changed = true;
                }
            } else {
                ui.actions.push(action);
            }
        }
        Ok(changed)
    }

    fn choose(&mut self, ui: &mut Ui, input: &mut InputState, index: usize) -> Result<()> {
        if let Some(option) = self.options.get(index).filter(|o| o.disabled.is_none()) {
            self.selected = Some(option.value.clone());
            self.changed = Some(option.value.clone());
            self.close(ui, input)?;
        }
        Ok(())
    }

    fn close(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<()> {
        self.open = false;
        self.layers.close(ui, input)?;
        if let Some(index) = ui.find(&self.popup_id()) {
            ui.set_visible(index, false)?;
        }
        if let Some(crate::widgets::Widget::Choice { open, .. }) =
            ui.widgets.get_mut(&stable_id(self.control_id()))
        {
            *open = false;
        }
        Ok(())
    }

    fn contains(&self, ui: &Ui, point: [f32; 2]) -> bool {
        ui.find(&self.popup_id())
            .is_some_and(|i| ui.shown(i) && ui.nodes[i].rect.contains(point[0], point[1]))
    }

    fn matching(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, o)| o.disabled.is_none() && o.label.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect()
    }

    fn repair_active(&mut self) {
        let matching = self.matching();
        if !self.active.is_some_and(|i| matching.contains(&i)) {
            self.active = matching.first().copied();
        }
    }

    fn reveal_active(&self, ui: &mut Ui) -> Result<()> {
        if let Some(index) = self.active.and_then(|i| ui.find(&self.option_id(i))) {
            ui.scroll_into_view(index)?;
        }
        Ok(())
    }

    /// Authored form-control ID, for native focus and host accessibility adapters.
    pub fn control_id(&self) -> &str {
        &self.control
    }
    fn popup_id(&self) -> String {
        format!("{}-popover", self.id)
    }
    fn search_id(&self) -> String {
        format!("{}-search", self.id)
    }
    fn option_id(&self, index: usize) -> String {
        format!("{}-option-{}-{index}", self.id, self.generation)
    }
    fn choose_action(&self) -> String {
        format!("ara.select.{}.choose", self.id)
    }
}

#[cfg(test)]
mod tests;
